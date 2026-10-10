//! SSH through the system `ssh` binary with one multiplexed connection, so a password is typed once at most.
use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use std::fs;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum, Debug)]
pub enum Auth {
    /// key already authorised on the router
    Key,
    /// ssh asks for the password itself (never stored)
    Password,
    /// password once, then install your public key
    CopyKey,
}

pub struct Out {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

impl Out {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

pub struct Router {
    pub host: String,
    pub user: String,
    pub port: u16,
    dir: PathBuf,
    batch: bool,
}

impl Router {
    pub fn new(host: &str, user: &str, port: u16, auth: Auth) -> Result<Router> {
        let dir = PathBuf::from(format!("/tmp/wf-{}", std::process::id()));
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
        Ok(Router {
            host: host.into(),
            user: user.into(),
            port,
            dir,
            batch: auth == Auth::Key,
        })
    }

    fn args(&self, batch: bool, mux: bool) -> Vec<String> {
        let mut a: Vec<String> = vec!["-p".into(), self.port.to_string()];
        let d = self.dir.display();
        let mut o = vec![
            "StrictHostKeyChecking=accept-new".to_string(),
            format!("UserKnownHostsFile={d}/kh"),
            "ServerAliveInterval=5".into(),
            "ConnectTimeout=8".into(),
            "LogLevel=ERROR".into(),
        ];
        if mux {
            o.extend([
                "ControlMaster=auto".to_string(),
                format!("ControlPath={d}/c"),
                "ControlPersist=300".into(),
            ]);
        } else {
            o.push("ControlPath=none".into());
        }
        if batch {
            o.push("BatchMode=yes".into());
        }
        for x in o {
            a.push("-o".into());
            a.push(x);
        }
        a.push(format!("{}@{}", self.user, self.host));
        a
    }

    /// Interactive on purpose: ssh itself asks for the password on the terminal.
    pub fn connect(&self) -> Result<()> {
        Command::new("ssh")
            .args(self.args(self.batch, true))
            .arg("true")
            .stdout(Stdio::null())
            .status()
            .context("could not start ssh; is OpenSSH installed?")?;
        if !self.run("true", None)?.ok {
            bail!("could not log in to {}@{}", self.user, self.host);
        }
        Ok(())
    }

    pub fn run(&self, cmd: &str, input: Option<&[u8]>) -> Result<Out> {
        self.exec(self.args(self.batch, true), cmd, input)
    }

    fn exec(&self, args: Vec<String>, cmd: &str, input: Option<&[u8]>) -> Result<Out> {
        let mut child = Command::new("ssh")
            .args(args)
            .arg(cmd)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start ssh")?;
        let mut stdin = child.stdin.take();
        let out = std::thread::scope(|s| {
            if let (Some(mut w), Some(data)) = (stdin.take(), input) {
                s.spawn(move || {
                    let _ = w.write_all(data); // dropped on exit, closing the pipe
                });
            }
            child.wait_with_output()
        })?;
        Ok(Out {
            ok: out.status.success(),
            stdout: out.stdout,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    /// Run and fail with the last lines of output when the command fails.
    pub fn check(&self, cmd: &str, input: Option<&[u8]>) -> Result<Out> {
        let o = self.run(cmd, input)?;
        if !o.ok {
            let all = format!("{}{}", o.stderr, o.text());
            let tail: Vec<&str> = all
                .trim()
                .lines()
                .rev()
                .take(6)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            bail!(
                "remote command failed ({}…)\n      {}",
                cmd.chars().take(40).collect::<String>(),
                tail.join("\n      ")
            );
        }
        Ok(o)
    }

    /// Run a script on the router via `sh -s` (no quoting trouble), stopping at the first error.
    pub fn sh(&self, script: &str) -> Result<String> {
        Ok(self
            .check("sh -s", Some(format!("set -e\n{script}").as_bytes()))?
            .text())
    }

    pub fn get(&self, remote: &str) -> Result<Vec<u8>> {
        Ok(self.check(&format!("cat {remote}"), None)?.stdout)
    }

    pub fn put(&self, data: &[u8], remote: &str) -> Result<()> {
        self.check(&format!("cat > {remote}"), Some(data))?;
        Ok(())
    }

    /// Drop the multiplexed connection (it dies on its own when the router reboots).
    pub fn reset(&self) {
        let _ = Command::new("ssh")
            .args(self.args(false, true))
            .args(["-O", "exit"])
            .output();
    }

    pub fn install_key(&mut self) -> Result<()> {
        let home = std::env::var("HOME").context("HOME is not set")?;
        let find = || {
            ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"]
                .iter()
                .map(|n| PathBuf::from(&home).join(".ssh").join(n))
                .find(|p| p.exists())
        };
        let pubfile = match find() {
            Some(p) => p,
            None => {
                let key = PathBuf::from(&home).join(".ssh/id_ed25519");
                if !Command::new("ssh-keygen")
                    .args(["-q", "-t", "ed25519", "-N", ""])
                    .arg("-f")
                    .arg(&key)
                    .status()?
                    .success()
                {
                    bail!("ssh-keygen failed");
                }
                key.with_extension("pub")
            }
        };
        let key = fs::read_to_string(pubfile)?.trim().to_string();
        self.sh(&format!(
            "mkdir -p /etc/dropbear; grep -qxF '{key}' /etc/dropbear/authorized_keys 2>/dev/null || echo '{key}' >> /etc/dropbear/authorized_keys; chmod 600 /etc/dropbear/authorized_keys"
        ))?;
        // prove key login works on a fresh connection, not through the password session
        if !self.exec(self.args(true, false), "true", None)?.ok {
            bail!("installed the key but key login still fails");
        }
        self.batch = true;
        Ok(())
    }
}

impl Drop for Router {
    fn drop(&mut self) {
        self.reset();
        let _ = fs::remove_dir_all(&self.dir);
    }
}
