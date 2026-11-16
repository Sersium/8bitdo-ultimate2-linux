use crate::{
    config::Config,
    device::Link,
    macros::{self, Macro},
    protocol::Result,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
const MAX_BACKUP_BYTES: usize = 2 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
pub struct Backup {
    pub format: u8,
    pub config: Vec<u8>,
    pub theme: u8,
    pub macros: Vec<Vec<Macro>>,
}
impl Backup {
    pub fn capture(l: &Link) -> Result<Self> {
        let c = l.read_config()?;
        let mut all = vec![];
        for s in 0..3 {
            let mut records = vec![];
            for i in 0..4 {
                records.push(macros::read(l, &c, s, i)?);
            }
            all.push(records);
        }
        Ok(Self {
            format: 1,
            config: c.0,
            theme: l.theme()?,
            macros: all,
        })
    }
    pub fn write(&self, path: &Path) -> Result<()> {
        let data = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())
    }
    pub fn validate(&self) -> Result<Config> {
        let c = Config::parse(self.config.clone())?;
        if self.format != 1
            || self.macros.len() != 3
            || self.macros.iter().any(|r| r.len() != 4)
            || !(2..=5).contains(&self.theme)
        {
            return Err("Invalid backup format".into());
        }
        for (s, row) in self.macros.iter().enumerate() {
            for (i, m) in row.iter().enumerate() {
                macros::validate(m)?;
                let o = 508 + s * 216 + i * 52;
                let active = c.u32(500 + s * 216) == crate::config::ENABLED && c.u32(o + 40) != 0;
                if (active
                    && (c.u16(o + 34) as usize != m.steps.len() || c.u32(o + 40) != m.trigger))
                    || (!active && !m.steps.is_empty())
                {
                    return Err("Backup macro data does not match its configuration".into());
                }
            }
        }
        Ok(c)
    }
}
pub fn directory() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share")
        })
        .join("ultimate2-native/backups")
}
pub fn automatic(l: &Link) -> Result<PathBuf> {
    let dir = directory();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let path = dir.join(format!("{now}.json"));
    Backup::capture(l)?.write(&path)?;
    Ok(path)
}
pub fn restore(l: &Link, path: &Path) -> Result<()> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take((MAX_BACKUP_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BACKUP_BYTES {
        return Err("Backup is too large".into());
    }
    let backup: Backup = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let target = backup.validate()?;
    let current = l.read_config()?;
    if target.u16(16) != current.u16(16) {
        return Err("Switch to the backup's input mode before restoring".into());
    }
    automatic(l)?;
    l.report(false)?;
    let result = (|| {
        for s in 0..3 {
            for i in 0..4 {
                let m = &backup.macros[s][i];
                if &macros::read(l, &current, s, i)? != m {
                    macros::write_steps(l, &current, s, i, m)?;
                }
            }
        }
        Ok(())
    })();
    let resume = l.report(true);
    result.and(resume)?;
    l.save_config(&current, &target)?;
    l.set_theme(backup.theme, l.info()?.edition)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backup_roundtrip_and_inconsistent_metadata_rejected() {
        let mut b = Backup {
            format: 1,
            config: vec![0; 1592],
            theme: 5,
            macros: vec![vec![Macro::default(); 4]; 3],
        };
        assert!(b.validate().is_ok());
        b.macros[0][0].steps.push(crate::macros::Step::default());
        b.macros[0][0].trigger = 1;
        assert!(b.validate().is_err());
        b.macros[0][0].steps.clear();
        let path = std::env::temp_dir().join(format!(
            "ultimate2-backup-{}-{}.json",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        b.write(&path).unwrap();
        assert!(b.write(&path).is_err());
        let got: Backup = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(got.validate().unwrap().0, b.config);
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn maximum_macro_backup_fits_the_restore_limit() {
        let mut c = Config(vec![0; 1592]);
        c.set16(16, 3);
        let m = Macro {
            name: "Full macro".into(),
            trigger: 1,
            cycles: 1,
            interval_ms: 0,
            steps: vec![crate::macros::Step::default(); 200],
        };
        for slot in 0..3 {
            for index in 0..4 {
                c = macros::metadata(&c, slot, index, &m).unwrap();
            }
        }
        let b = Backup {
            format: 1,
            config: c.0,
            theme: 5,
            macros: vec![vec![m; 4]; 3],
        };
        b.validate().unwrap();
        assert!(serde_json::to_vec_pretty(&b).unwrap().len() < MAX_BACKUP_BYTES);
    }
    #[test]
    fn malformed_backups_rejected() {
        let b = Backup {
            format: 2,
            config: vec![0; 1592],
            theme: 5,
            macros: vec![],
        };
        assert!(b.validate().is_err());
    }
}
