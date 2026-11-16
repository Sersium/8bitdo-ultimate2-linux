//! Native single-image Telink update path used by the vendor's WebHID updater.
use crate::{
    device::{Link, nodes},
    protocol::{self, Packet, Result, Transport, dword},
};
use serde::Deserialize;
use std::{
    io::Read,
    thread,
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Controller,
    Receiver,
}
impl Target {
    pub fn pid(self) -> u16 {
        match self {
            Self::Controller => 0x6012,
            Self::Receiver => 0x6013,
        }
    }
    pub fn firmware_type(self) -> u8 {
        match self {
            Self::Controller => 94,
            Self::Receiver => 95,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Controller => "Controller",
            Self::Receiver => "Dongle",
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Versions {
    pub controller: Option<u32>,
    pub dongle: Option<u32>,
    pub controller_boot: bool,
    pub dongle_boot: bool,
}
impl Versions {
    pub fn get(self, target: Target) -> Option<u32> {
        match target {
            Target::Controller => self.controller,
            Target::Receiver => self.dongle,
        }
    }
    pub fn boot(self, target: Target) -> bool {
        match target {
            Target::Controller => self.controller_boot,
            Target::Receiver => self.dongle_boot,
        }
    }
    pub fn check_update(self, target: Target, wanted: u32) -> Result<()> {
        match self.get(target) {
            Some(current) if current == wanted => Err(format!(
                "{} firmware {} is already installed",
                target.name(),
                version_text(current)
            )),
            None if !self.boot(target) => Err(format!(
                "Read the {} firmware first: connect USB and use XInput, then refresh",
                target.name()
            )),
            _ => Ok(()),
        }
    }
}
pub fn version_text(v: u32) -> String {
    format!("{}.{:02}", v / 100, v % 100)
}
pub fn version_number(text: &str) -> Option<u32> {
    let (a, b) = text.split_once('.')?;
    let major: u32 = a.parse().ok()?;
    let minor: u32 = b.parse().ok()?;
    if minor >= 100 {
        return None;
    }
    major.checked_mul(100)?.checked_add(minor)
}
pub fn versions() -> Result<Versions> {
    let mut out = Versions::default();
    for n in nodes()?.iter().filter(|n| n.config) {
        let Ok(l) = Link::open(n) else {
            continue;
        };
        let Ok(pid) = identify(&l) else {
            continue;
        };
        if [0x3206, 0x3208].contains(&n.pid) {
            if pid == Target::Controller.pid() {
                out.controller_boot = true;
            }
            if pid == Target::Receiver.pid() {
                out.dongle_boot = true;
            }
            continue;
        }
        if let Ok(r) = l.raw_query(&[0x81, 5, 0, 0x21, 1], &[2, 0x22])
            && r.len() >= 4
        {
            let v = protocol::word(&r, 2) as u32;
            if (1..=9999).contains(&v) && pid == Target::Controller.pid() {
                out.controller = Some(v);
            }
            if (1..=9999).contains(&v) && pid == Target::Receiver.pid() {
                out.dongle = Some(v);
            }
        }
        if out.controller.is_none()
            && pid == Target::Receiver.pid()
            && let Ok(r) = l.raw_query(&[0x81, 4, 0x21, 1, 0, 0, 6], &[2, 4, 4, 0, 0x21, 1])
            && r.len() > 23
            && protocol::word(&r, 19) > 0
        {
            out.controller = Some(protocol::word(&r, 19) as u32);
        }
    }
    Ok(out)
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    #[serde(rename = "file_version")]
    pub version: String,
    #[serde(rename = "fileURL")]
    pub url: String,
    #[serde(rename = "fileSize")]
    pub size: usize,
    #[serde(default)]
    pub readme_en: String,
    pub firmware_type: u8,
}
#[derive(Deserialize)]
struct Feed {
    code: u16,
    data: Vec<Release>,
}
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .https_only(true)
        .max_redirects(0)
        .build()
        .into()
}
pub fn releases(target: Target) -> Result<Vec<Release>> {
    let feed: Feed = agent()
        .post("https://support.8bitdo.com/restapi/link/v1/client/firmware/select")
        .send_json(serde_json::json!({"firmware_type":target.firmware_type(),"beta":0}))
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    if feed.code != 200
        || feed
            .data
            .iter()
            .any(|r| r.firmware_type != target.firmware_type())
    {
        return Err("Invalid official firmware response".into());
    }
    Ok(feed.data)
}
#[derive(Clone, Debug)]
pub struct Image {
    pub target: Target,
    pub version: u32,
    pub address: u32,
    pub bytes: Vec<u8>,
}
impl Image {
    pub fn parse(bytes: Vec<u8>, target: Target) -> Result<Self> {
        if bytes.len() < 60 || bytes.len() > 524_316 {
            return Err("Firmware image size is invalid".into());
        }
        let version = dword(&bytes, 0);
        let address = dword(&bytes, 4);
        let n = dword(&bytes, 8) as usize;
        if dword(&bytes, 12) != target.pid() as u32 {
            return Err("Firmware target does not match the selected controller/dongle".into());
        }
        if address != 0x5000
            || n + 28 != bytes.len()
            || !(1..=9999).contains(&version)
            || bytes[36..40] != *b"KNLT"
            || dword(&bytes, 20) != 0
            || dword(&bytes, 24) != 0
        {
            return Err("Unsupported firmware layout; no data written".into());
        }
        Ok(Self {
            target,
            version,
            address,
            bytes,
        })
    }
}
pub fn download(r: &Release, target: Target) -> Result<Image> {
    if !r.url.starts_with("https://support.8bitdo.com/bd-uploads/")
        || r.size > 524_316
        || r.firmware_type != target.firmware_type()
    {
        return Err("Untrusted firmware URL or target".into());
    }
    let mut response = agent()
        .get(&r.url)
        .header("Referer", "https://web.8bitdo.com/")
        .header("User-Agent", "Mozilla/5.0 Ultimate2-Native")
        .call()
        .map_err(|e| e.to_string())?;
    let mut data = vec![];
    response
        .body_mut()
        .as_reader()
        .take(524_317)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() != r.size {
        return Err("Firmware download is incomplete".into());
    }
    let image = Image::parse(data, target)?;
    let version = format!("{}.{:02}", image.version / 100, image.version % 100);
    if version != r.version {
        return Err("Firmware version does not match the official feed".into());
    }
    Ok(image)
}
pub fn identify(t: &impl Transport) -> Result<u16> {
    let mut p = Packet::new(5, 8);
    p.length = 4;
    p.total = 4;
    let r = t.request(p, Duration::from_millis(600))?;
    if r.len() < 2 {
        return Err("Bootloader returned no target identity".into());
    }
    Ok(protocol::word(&r, 0))
}
pub fn boot(target: Target) -> Result<Link> {
    // Query the device itself: 310b is shared by the wired pad and receiver.
    let mut selected = None;
    for n in nodes()?.into_iter().filter(|n| n.config) {
        let l = Link::open(&n)?;
        if identify(&l).ok() == Some(target.pid()) {
            selected = Some(l);
            break;
        }
    }
    let link = selected.ok_or_else(|| format!("Connect the {} directly by USB", target.name()))?;
    if [0x3206, 0x3208].contains(&link.node.pid) {
        return Ok(link);
    }
    let mut p = [0; 64];
    p[..5].copy_from_slice(&[0x81, 5, 0, 0x51, 0]);
    link.write(&p)?;
    drop(link);
    let end = Instant::now() + Duration::from_secs(12);
    while Instant::now() < end {
        for n in nodes()?
            .into_iter()
            .filter(|n| [0x3206, 0x3208].contains(&n.pid))
        {
            let l = Link::open(&n)?;
            if identify(&l).ok() == Some(target.pid()) {
                return Ok(l);
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err("Bootloader did not appear. Reconnect USB while holding LB+RB, then retry.".into())
}
pub fn flash(t: &impl Transport, image: &Image, mut progress: impl FnMut(f32)) -> Result<()> {
    if identify(t)? != image.target.pid() {
        return Err("Bootloader target mismatch; no flash erased".into());
    }
    let payload = &image.bytes[28..];
    let mut erase = Packet::new(5, 4);
    erase.total = payload.len() as u32;
    erase.offset = image.address;
    t.request(erase, Duration::from_secs(10))?;
    progress(0.);
    for (i, bytes) in payload.chunks(46).enumerate() {
        let mut p = Packet::new(5, 3);
        p.offset = image.address + (i * 46) as u32;
        p.total = payload.len() as u32;
        p.data = bytes.to_vec();
        p.length = bytes.len();
        t.request(p, Duration::from_secs(2))?;
        progress(((i * 46 + bytes.len()) as f32 / payload.len() as f32) * 0.95);
    }
    let mut save = Packet::new(5, 0xc4);
    save.data = (payload.len() as u32)
        .to_le_bytes()
        .into_iter()
        .chain(image.version.to_le_bytes())
        .collect();
    save.length = 8;
    t.request(save, Duration::from_secs(3))?;
    t.write(&Packet::new(5, 7).encode()?)?;
    progress(1.);
    Ok(())
}
pub fn verify(target: Target, version: u32) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(12);
    while Instant::now() < end {
        for n in nodes()?
            .into_iter()
            .filter(|n| n.config && ![0x3206, 0x3208].contains(&n.pid))
        {
            let l = Link::open(&n)?;
            if identify(&l).ok() != Some(target.pid()) {
                continue;
            }
            if let Ok(r) = l.raw_query(&[0x81, 5, 0, 0x21, 1], &[2, 0x22])
                && r.len() >= 4
                && protocol::word(&r, 2) as u32 == version
            {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err("Firmware was sent, but the running version could not be verified. Keep the device connected and retry recovery from boot mode.".into())
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    fn image(target: Target) -> Image {
        let mut b = vec![0; 124];
        for (o, n) in [(0, 110u32), (4, 0x5000), (8, 96), (12, target.pid() as u32)] {
            b[o..o + 4].copy_from_slice(&n.to_le_bytes());
        }
        b[36..40].copy_from_slice(b"KNLT");
        Image::parse(b, target).unwrap()
    }
    struct Fake {
        pid: u16,
        log: RefCell<Vec<Packet>>,
        writes: RefCell<Vec<Vec<u8>>>,
    }
    impl Transport for Fake {
        fn write(&self, b: &[u8]) -> Result<()> {
            self.writes.borrow_mut().push(b.to_vec());
            Ok(())
        }
        fn read(&self, _: i32) -> Result<Vec<u8>> {
            unreachable!()
        }
        fn request(&self, p: Packet, _: Duration) -> Result<Vec<u8>> {
            let r = if p.cmd == 8 {
                (self.pid as u32).to_le_bytes().to_vec()
            } else {
                vec![]
            };
            self.log.borrow_mut().push(p);
            Ok(r)
        }
    }
    #[test]
    fn same_version_is_blocked_for_both_devices() {
        let v = Versions {
            controller: Some(110),
            dongle: Some(106),
            ..Default::default()
        };
        for (target, number) in [(Target::Controller, 110), (Target::Receiver, 106)] {
            assert!(v.check_update(target, number).is_err());
            assert!(v.check_update(target, number + 1).is_ok());
            assert!(Versions::default().check_update(target, number).is_err());
        }
        assert!(
            Versions {
                controller_boot: true,
                ..Default::default()
            }
            .check_update(Target::Controller, 110)
            .is_ok()
        );
        assert_eq!(version_number("1.10"), Some(110));
        assert_eq!(version_number("invalid"), None);
    }
    #[test]
    fn both_targets_and_wrong_target_protection() {
        for target in [Target::Controller, Target::Receiver] {
            let im = image(target);
            let f = Fake {
                pid: target.pid(),
                log: RefCell::new(vec![]),
                writes: RefCell::new(vec![]),
            };
            let mut progress = vec![];
            flash(&f, &im, |p| progress.push(p)).unwrap();
            let log = f.log.borrow();
            assert_eq!(log[1].cmd, 4);
            assert_eq!(log[2].cmd, 3);
            assert_eq!(log[2].offset, 0x5000);
            assert_eq!(log.last().unwrap().cmd, 0xc4);
            assert_eq!(progress.last(), Some(&1.));
            assert_eq!(f.writes.borrow()[0][2], 7);
        }
        let f = Fake {
            pid: 0x6013,
            log: RefCell::new(vec![]),
            writes: RefCell::new(vec![]),
        };
        assert!(flash(&f, &image(Target::Controller), |_| {}).is_err());
        assert_eq!(f.log.borrow().len(), 1);
    }
    #[test]
    fn truncated_or_mismatched_image_refused() {
        let im = image(Target::Controller);
        assert!(Image::parse(im.bytes.clone(), Target::Receiver).is_err());
        assert!(Image::parse(im.bytes[..50].to_vec(), Target::Controller).is_err());
        let mut b = im.bytes;
        b[5] = 0;
        assert!(Image::parse(b, Target::Controller).is_err());
    }
}
