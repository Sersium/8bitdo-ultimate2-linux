use crate::{
    config::{Config, SIZE},
    protocol::{Packet, Result, Transport, word},
};
use hidapi::{HidApi, HidDevice};
use std::{
    ffi::CString,
    fs::{self, File},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub path: CString,
    pub pid: u16,
    pub name: String,
    pub port: String,
    pub config: bool,
    pub motion: bool,
}
fn usb_root(path: &str) -> Option<std::path::PathBuf> {
    let name = path.rsplit('/').next()?;
    fs::canonicalize(format!("/sys/class/hidraw/{name}/device"))
        .ok()
        .and_then(|p| {
            p.ancestors()
                .find(|a| a.join("idVendor").exists())
                .map(|p| p.to_owned())
        })
}
pub fn nodes() -> Result<Vec<Node>> {
    let api = HidApi::new().map_err(|e| e.to_string())?;
    let mut out = vec![];
    for d in api.device_list().filter(|d| d.vendor_id() == 0x2dc8) {
        let name = d.product_string().unwrap_or("");
        if !([0x6012, 0x6013, 0x3107, 0x3206, 0x3208].contains(&d.product_id())
            || (d.product_id() == 0x310b && name.contains("Ultimate 2") && !name.contains("2C")))
        {
            continue;
        }
        let mut desc = [0; 4096];
        let n = d
            .open_device(&api)
            .and_then(|h| h.get_report_descriptor(&mut desc))
            .unwrap_or(0);
        let config = [0x3206, 0x3208].contains(&d.product_id())
            || desc[..n].windows(2).any(|v| v == [0x85, 0x81]);
        let port = usb_root(&d.path().to_string_lossy())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(Node {
            path: d.path().to_owned(),
            pid: d.product_id(),
            name: name.into(),
            port,
            config,
            motion: d.product_id() == 0x6012 && desc[..n].starts_with(&[5, 1]),
        });
    }
    out.sort_by_key(|d| {
        (
            d.pid != 0x310b,
            d.pid == 0x6013,
            d.path.to_string_lossy().into_owned(),
        )
    });
    out.dedup_by(|a, b| a.path == b.path);
    Ok(out)
}

fn settings_ranges(first: usize, end: usize) -> [(usize, usize); 2] {
    [(first, end.min(12)), (first.max(20), end)]
}
pub struct Link {
    pub node: Node,
    pub hid: HidDevice,
    _poll: Option<File>,
}
impl Link {
    pub fn open(node: &Node) -> Result<Self> {
        let api = HidApi::new().map_err(|e| e.to_string())?;
        let hid = api.open_path(&node.path).map_err(|e| {
            format!(
                "{}: {e}. Check your 8BitDo udev permissions.",
                node.path.to_string_lossy()
            )
        })?;
        let root = usb_root(&node.path.to_string_lossy());
        let poll = root.and_then(|root| {
            fs::read_dir("/sys/class/input").ok().and_then(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.file_name().to_string_lossy().starts_with("js"))
                    .find_map(|e| {
                        fs::canonicalize(e.path().join("device"))
                            .ok()
                            .filter(|p| p.starts_with(&root))
                            .and_then(|_| {
                                File::open(format!(
                                    "/dev/input/{}",
                                    e.file_name().to_string_lossy()
                                ))
                                .ok()
                            })
                    })
            })
        });
        Ok(Self {
            node: node.clone(),
            hid,
            _poll: poll,
        })
    }
    pub fn connect() -> Result<Self> {
        let ns = nodes()?;
        let mut wireless = None;
        for n in ns
            .iter()
            .filter(|d| d.config && ![0x3206, 0x3208].contains(&d.pid))
        {
            let l = Self::open(n)?;
            if l.info().is_ok() && l.read_config().is_ok() {
                if crate::firmware::identify(&l).ok() == Some(0x6012) {
                    return Ok(l);
                }
                if wireless.is_none() {
                    wireless = Some(l);
                }
            }
        }
        wireless.ok_or_else(|| {
            "Connect the Ultimate 2 directly by USB, or use its receiver in XInput mode.".into()
        })
    }
    pub fn raw_query(&self, bytes: &[u8], prefix: &[u8]) -> Result<Vec<u8>> {
        let mut p = [0; 64];
        if bytes.len() > 64 {
            return Err("Invalid query length".into());
        }
        p[..bytes.len()].copy_from_slice(bytes);
        self.write(&p)?;
        let end = Instant::now() + Duration::from_millis(400);
        while Instant::now() < end {
            let r = self.read(50)?;
            if r.starts_with(prefix) {
                return Ok(r);
            }
        }
        Err("Controller query timed out".into())
    }
    pub fn info(&self) -> Result<Info> {
        let r = self.raw_query(&[0x81, 4, 0, 1], &[2, 4, 4, 0, 0, 1])?;
        if r.len() < 24 {
            return Err("Short controller info report".into());
        }
        let mut info = Info {
            firmware: word(&r, 18),
            edition: 0,
            battery: None,
            path: self.node.path.to_string_lossy().into_owned(),
        };
        if let Ok(r) = self.raw_query(&[0x81, 5, 0, 0x21, 1], &[2, 0x22])
            && r.len() > 6
        {
            info.edition = r[6];
        }
        if let Ok(r) = self.raw_query(&[0x81, 4, 0x21, 1, 0, 0, 6], &[2, 4, 4, 0, 0x21, 1])
            && r.len() > 23
        {
            info.firmware = word(&r, 19);
            info.battery = Some(r[18].min(100));
            info.edition = r[23];
        }
        Ok(info)
    }
    pub fn read_config(&self) -> Result<Config> {
        let mut data = vec![];
        while data.len() < SIZE {
            let mut p = Packet::new(4, 2);
            p.total = SIZE as u32;
            p.offset = data.len() as u32;
            p.length = (SIZE - data.len()).min(45);
            let r = self.request(p.clone(), Duration::from_secs(1))?;
            if r.is_empty() || r.len() > p.length {
                return Err("Invalid configuration chunk".into());
            }
            data.extend(r);
        }
        Config::parse(data)
    }
    pub fn report(&self, on: bool) -> Result<()> {
        let mut p = Packet::new(4, 7);
        p.sub = u16::from(on);
        self.request(p, Duration::from_millis(500)).map(|_| ())
    }
    pub fn save_config(&self, old: &Config, new: &Config) -> Result<()> {
        let current = self.read_config()?;
        if current.u16(16) != old.u16(16) {
            return Err("Input mode changed. Reconnect before saving this profile.".into());
        }
        if current.0[..12] != old.0[..12] || current.0[20..] != old.0[20..] {
            return Err("Controller settings changed. Reload before saving.".into());
        }
        let indices: Vec<usize> = (0..SIZE)
            .filter(|&i| !(12..20).contains(&i) && old.0[i] != new.0[i])
            .collect();
        let Some(&first) = indices.first() else {
            return Ok(());
        };
        let end = indices[indices.len() - 1] + 1;
        self.report(false)?;
        let result = (|| {
            for (start, stop) in settings_ranges(first, end) {
                for o in (start..stop).step_by(45) {
                    let mut p = Packet::new(4, 1);
                    p.offset = o as u32;
                    p.total = SIZE as u32;
                    p.data = new.0[o..stop.min(o + 45)].to_vec();
                    p.length = p.data.len();
                    self.request(p, Duration::from_secs(1))?;
                }
            }
            let mut p = Packet::new(4, 6);
            p.sub = 0x123;
            self.request(p, Duration::from_secs(3))?;
            let got = self.read_config()?;
            if got.0[..12] != new.0[..12] || got.0[20..] != new.0[20..] {
                return Err("Configuration verification failed".into());
            }
            Ok(())
        })();
        let resume = self.report(true);
        result.and(resume)
    }
    pub fn theme(&self) -> Result<u8> {
        let r = self.request(Packet::new(4, 0x41), Duration::from_millis(500))?;
        let t = *r.first().ok_or("Empty lighting response")?;
        Ok(if t == 6 { 5 } else { t })
    }
    pub fn set_theme(&self, theme: u8, edition: u8) -> Result<()> {
        if !(2..=5).contains(&theme) {
            return Err("Unsupported lighting theme".into());
        }
        let mut p = Packet::new(4, 0x40);
        p.sub = if theme == 5 && edition == 2 {
            6
        } else {
            theme as u16
        };
        self.request(p, Duration::from_millis(500))?;
        if self.theme()? != theme {
            return Err("Lighting setting did not persist".into());
        }
        Ok(())
    }
    pub fn rumble(&self, left: f32, right: f32, ms: u16) -> Result<()> {
        if ms > 2000 || !left.is_finite() || !right.is_finite() {
            return Err("Invalid rumble preview".into());
        }
        if self.node.motion && !self.node.config {
            return self.write(&[
                5,
                (left.clamp(0., 1.) * 255.) as u8,
                (right.clamp(0., 1.) * 255.) as u8,
                0,
                0,
            ]);
        }
        let mut b = [0; 64];
        b[..9].copy_from_slice(&[
            0x81,
            0x11,
            4,
            8,
            0,
            ms as u8,
            (ms >> 8) as u8,
            (left.clamp(0., 1.) * 255.) as u8,
            (right.clamp(0., 1.) * 255.) as u8,
        ]);
        // The firmware reads duration at bytes 5–6 and motor strength at 7–8.
        self.write(&b)
    }
    pub fn for_rumble() -> Result<Self> {
        Self::connect().or_else(|_| {
            let n = nodes()?
                .into_iter()
                .find(|n| n.motion && !n.config)
                .ok_or("Connect the controller by USB or its DInput receiver")?;
            Self::open(&n)
        })
    }
}
impl Transport for Link {
    fn write(&self, b: &[u8]) -> Result<()> {
        let mut bytes = b.to_vec();
        if [0x3206, 0x3208].contains(&self.node.pid) {
            bytes[0] = 0; // Boot USB has no report IDs; hidapi requires a zero prefix.
            bytes.push(0);
        }
        let n = self.hid.write(&bytes).map_err(|e| e.to_string())?;
        if n != bytes.len() {
            Err("Incomplete HID write".into())
        } else {
            Ok(())
        }
    }
    fn read(&self, timeout_ms: i32) -> Result<Vec<u8>> {
        let mut b = [0; 64];
        let n = self
            .hid
            .read_timeout(&mut b, timeout_ms)
            .map_err(|e| e.to_string())?;
        if n > 0 && [0x3206, 0x3208].contains(&self.node.pid) {
            b[0] = 2;
        }
        Ok(b[..n].to_vec())
    }
}
#[derive(Clone, Debug)]
pub struct Info {
    pub firmware: u16,
    pub edition: u8,
    pub battery: Option<u8>,
    pub path: String,
}

pub fn wait_pair(cancel: &std::sync::atomic::AtomicBool) -> Result<()> {
    let ns = nodes()?;
    let receiver = ns
        .iter()
        .find(|n| [0x6013, 0x3107].contains(&n.pid))
        .ok_or("Connect the receiver while the controller is off, then start pairing")?;
    let port = receiver.port.clone();
    let end = Instant::now() + Duration::from_secs(45);
    while Instant::now() < end && !cancel.load(std::sync::atomic::Ordering::Relaxed) {
        for n in nodes()?.into_iter().filter(|n| n.port == port && n.config) {
            let l = Link::open(&n)?;
            let r = l.raw_query(&[0x81, 4, 0x21, 1, 0, 0, 6], &[2, 4, 4, 0, 0x21, 1]);
            if r.is_ok_and(|r| r.len() > 23 && word(&r, 19) > 0) {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err("Pairing was cancelled or timed out; hold Pair for 3 seconds and retry".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_writes_never_overwrite_runtime_fields() {
        let mut got = vec![255u8; SIZE];
        let new = vec![0u8; SIZE];
        for (start, end) in settings_ranges(0, SIZE) {
            got[start..end].copy_from_slice(&new[start..end]);
        }
        assert_eq!(&got[12..20], &[255; 8]);
        assert!(got[..12].iter().all(|&b| b == 0));
        assert!(got[20..].iter().all(|&b| b == 0));
    }
}
