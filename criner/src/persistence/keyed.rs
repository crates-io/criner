use crate::model::{Context, Crate, CrateVersion, Task, TaskResult};
use std::time::SystemTime;

pub const KEY_SEP_CHAR: char = ':';

pub trait Keyed {
    fn key_buf(&self, buf: &mut String);
    fn key(&self) -> String {
        let mut buf = String::with_capacity(16);
        self.key_buf(&mut buf);
        buf
    }
}

impl Keyed for Task {
    fn key_buf(&self, buf: &mut String) {
        buf.push_str(&self.process);
        buf.push(KEY_SEP_CHAR);
        buf.push_str(&self.version);
    }
}

impl Task {
    pub fn fq_key(&self, crate_name: &str, crate_version: &str, buf: &mut String) {
        CrateVersion::key_from(crate_name, crate_version, buf);
        buf.push(KEY_SEP_CHAR);
        self.key_buf(buf);
    }
}

impl Keyed for CrateVersion {
    fn key_buf(&self, buf: &mut String) {
        CrateVersion::key_from(&self.name, &self.version, buf)
    }
}

impl Crate {
    pub fn key_from_version_buf(v: &CrateVersion, buf: &mut String) {
        buf.push_str(&v.name);
    }
}

impl Keyed for TaskResult {
    fn key_buf(&self, buf: &mut String) {
        match self {
            TaskResult::Download { kind, .. } => {
                buf.push(KEY_SEP_CHAR);
                buf.push_str(kind)
            }
            TaskResult::None | TaskResult::ExplodedCrate { .. } => {}
        }
    }
}

impl TaskResult {
    pub fn fq_key(&self, crate_name: &str, crate_version: &str, task: &Task, buf: &mut String) {
        task.fq_key(crate_name, crate_version, buf);
        self.key_buf(buf);
    }
}

impl Keyed for Context {
    fn key_buf(&self, buf: &mut String) {
        use std::fmt::Write;
        write!(
            buf,
            "context/{}",
            humantime::format_rfc3339(SystemTime::now())
                .to_string()
                .get(..10)
                .expect("YYYY-MM-DD - 10 bytes")
        )
        .ok();
    }
}

impl CrateVersion {
    pub fn key_from(name: &str, version: &str, buf: &mut String) {
        buf.push_str(name);
        buf.push(KEY_SEP_CHAR);
        buf.push_str(version);
    }
}
