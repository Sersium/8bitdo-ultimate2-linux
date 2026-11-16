use crate::{
    config::{Config, ENABLED},
    protocol::{Packet, Result, Transport, word},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub duration_ms: u16,
    pub keys: u16,
    pub triggers: [u8; 2],
    pub left: [u8; 2],
    pub right: [u8; 2],
}
impl Default for Step {
    fn default() -> Self {
        Self {
            duration_ms: 100,
            keys: 0,
            triggers: [0, 0],
            left: [127, 127],
            right: [127, 127],
        }
    }
}
impl Step {
    pub fn bytes(&self) -> [u8; 10] {
        let mut b = [0; 10];
        b[..2].copy_from_slice(&self.duration_ms.to_le_bytes());
        b[2..4].copy_from_slice(&self.keys.to_le_bytes());
        b[4] = self.triggers[1];
        b[5] = self.triggers[0];
        b[6..8].copy_from_slice(&self.left);
        b[8..10].copy_from_slice(&self.right);
        b
    }
    pub fn parse(b: &[u8]) -> Self {
        Self {
            duration_ms: word(b, 0),
            keys: word(b, 2),
            triggers: [b[5], b[4]],
            left: [b[6], b[7]],
            right: [b[8], b[9]],
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    pub name: String,
    pub trigger: u32,
    pub cycles: u32,
    pub interval_ms: u32,
    pub steps: Vec<Step>,
}
pub const TRIGGERS: [u32; 22] = [
    8192, 4096, 16, 32, 512, 256, 128, 64, 33554432, 67108864, 2097152, 1073741824, 1048576, 1024,
    16384, 2, 2048, 32768, 4, 8, 1, 65536,
];
impl Default for Macro {
    fn default() -> Self {
        Self {
            name: "Macro".into(),
            trigger: 0,
            cycles: 1,
            interval_ms: 0,
            steps: vec![],
        }
    }
}
pub fn validate(m: &Macro) -> Result<()> {
    if m.name.encode_utf16().count() > 16
        || m.steps.len() > 200
        || m.cycles > 1000
        || m.interval_ms > 60000
    {
        return Err(
            "Macro limit: 16 name characters, 200 steps, 1000 repeats, 60000 ms interval".into(),
        );
    }
    if (m.trigger != 0 && !TRIGGERS.contains(&m.trigger)) || (m.trigger == 0 && !m.steps.is_empty())
    {
        return Err("Choose a macro trigger button".into());
    }
    if m.steps.iter().any(|s| s.duration_ms == 0) {
        return Err("Step duration must be at least 1 ms".into());
    }
    Ok(())
}
fn address(c: &Config, slot: usize, index: usize) -> Result<(u16, u32)> {
    if slot > 2 || index > 3 || ![0, 1, 3].contains(&c.u16(16)) {
        return Err("Invalid macro slot or input mode".into());
    }
    Ok(((c.u16(16) << 8) | slot as u16, index as u32 * 4096))
}
pub fn read(t: &impl Transport, c: &Config, slot: usize, index: usize) -> Result<Macro> {
    let (sub, offset) = address(c, slot, index)?;
    let o = 508 + slot * 216 + index * 52;
    if c.u32(500 + slot * 216) != ENABLED || c.u32(o + 40) == 0 {
        return Ok(Macro::default());
    }
    let count = c.u16(o + 34) as usize;
    if count > 200 {
        return Err("Controller macro exceeds 200 steps".into());
    }
    let mut data = vec![];
    while data.len() < count * 10 {
        let mut p = Packet::new(4, 0x102);
        p.sub = sub;
        p.offset = offset + data.len() as u32;
        p.total = (count * 10) as u32;
        p.length = (count * 10 - data.len()).min(32);
        let r = t.request(p.clone(), Duration::from_secs(1))?;
        if r.is_empty() || r.len() > p.length {
            return Err("Invalid macro chunk".into());
        }
        data.extend(r);
    }
    let name = String::from_utf16_lossy(
        &c.0[o..o + 32]
            .chunks_exact(2)
            .map(|b| u16::from_be_bytes([b[0], b[1]]))
            .take_while(|&x| x != 0 && x != 0xffff)
            .collect::<Vec<_>>(),
    );
    Ok(Macro {
        name,
        trigger: c.u32(o + 40),
        cycles: c.u32(o + 44),
        interval_ms: c.u32(o + 48),
        steps: data.chunks_exact(10).map(Step::parse).collect(),
    })
}
pub fn write_steps(
    t: &impl Transport,
    c: &Config,
    slot: usize,
    index: usize,
    m: &Macro,
) -> Result<()> {
    validate(m)?;
    let (sub, offset) = address(c, slot, index)?;
    let data: Vec<u8> = m.steps.iter().flat_map(Step::bytes).collect();
    let padded = (data.len() + 31) & !31;
    let mut bytes = data.clone();
    bytes.resize(padded, 0);
    let mut p = Packet::new(4, 0x104);
    p.sub = sub;
    p.offset = offset;
    p.total = 4096;
    t.request(p, Duration::from_secs(5))?;
    for (i, part) in bytes.chunks(32).enumerate() {
        let mut p = Packet::new(4, 0x103);
        p.sub = sub;
        p.offset = offset + (i * 32) as u32;
        p.total = offset + padded as u32;
        p.data = part.to_vec();
        p.length = p.data.len();
        t.request(p, Duration::from_secs(1))?;
    }
    let meta = metadata(c, slot, index, m)?;
    let got = read(t, &meta, slot, index)?;
    if got.steps != m.steps {
        return Err("Macro step verification failed; restore the automatic backup".into());
    }
    Ok(())
}
pub fn metadata(c: &Config, slot: usize, index: usize, m: &Macro) -> Result<Config> {
    validate(m)?;
    address(c, slot, index)?;
    let mut new = c.clone();
    new.enable_slot(slot);
    for i in 0..4 {
        let r = 508 + slot * 216 + i * 52;
        if new.u16(r + 34) == u16::MAX && new.u32(r + 40) == u32::MAX {
            new.0[r..r + 52].fill(0);
        }
    }
    let o = 508 + slot * 216 + index * 52;
    new.set32(slot * 4, ENABLED);
    new.set32(500 + slot * 216, ENABLED);
    new.0[o..o + 52].fill(0);
    for (i, ch) in m.name.encode_utf16().enumerate() {
        new.0[o + 2 * i..o + 2 * i + 2].copy_from_slice(&ch.to_be_bytes());
    }
    new.0[o + 32] = c.u16(16) as u8;
    new.set16(o + 34, m.steps.len() as u16);
    new.set32(o + 40, if m.steps.is_empty() { 0 } else { m.trigger });
    new.set32(o + 44, m.cycles);
    new.set32(o + 48, m.interval_ms);
    new.0[504 + slot * 216] = (0..4)
        .filter(|i| {
            let o = 508 + slot * 216 + i * 52;
            let count = new.u16(o + 34);
            (1..=200).contains(&count) && TRIGGERS.contains(&new.u32(o + 40))
        })
        .count() as u8;
    Ok(new)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    struct Fake(RefCell<Vec<u8>>, RefCell<Vec<Packet>>);
    impl Transport for Fake {
        fn write(&self, _: &[u8]) -> Result<()> {
            unreachable!()
        }
        fn read(&self, _: i32) -> Result<Vec<u8>> {
            unreachable!()
        }
        fn request(&self, p: Packet, _: Duration) -> Result<Vec<u8>> {
            let o = (p.offset % 4096) as usize;
            let mut b = self.0.borrow_mut();
            let out = match p.cmd {
                0x104 => {
                    b.fill(255);
                    vec![]
                }
                0x103 => {
                    b[o..o + p.length].copy_from_slice(&p.data);
                    vec![]
                }
                0x102 => b[o..o + p.length].to_vec(),
                _ => panic!(),
            };
            self.1.borrow_mut().push(p);
            Ok(out)
        }
    }
    #[test]
    fn macro_roundtrip_and_addressing() {
        let mut c = Config(vec![0; 1592]);
        c.set16(16, 3);
        let f = Fake(RefCell::new(vec![0; 4096]), RefCell::new(vec![]));
        let m = Macro {
            name: "Combo".into(),
            trigger: 0x2000000,
            cycles: 2,
            interval_ms: 5,
            steps: vec![
                Step {
                    keys: 0x2010,
                    triggers: [125, 255],
                    left: [1, 128],
                    ..Step::default()
                };
                5
            ],
        };
        write_steps(&f, &c, 2, 3, &m).unwrap();
        let new = metadata(&c, 2, 3, &m).unwrap();
        assert_eq!(read(&f, &new, 2, 3).unwrap(), m);
        let log = f.1.borrow();
        assert_eq!(log[0].sub, 0x302);
        assert_eq!(log[0].offset, 12288);
        assert_eq!(log[1].total, 12288 + 64);
        assert_eq!(new.0[936], 1);
        assert_eq!(Step::parse(&m.steps[0].bytes()), m.steps[0]);
        assert!(address(&c, 3, 0).is_err());
        let erased = Config(vec![255; 1592]);
        let mut erased = erased;
        erased.set16(16, 3);
        let new = metadata(&erased, 0, 0, &m).unwrap();
        assert_eq!(new.0[504], 1);
        assert_eq!(new.u16(508 + 52 + 34), 0);
        assert_eq!(new.u32(4), u32::MAX);
    }
    #[test]
    fn bad_macro_is_rejected_before_write() {
        let mut m = Macro {
            steps: vec![Step::default()],
            ..Macro::default()
        };
        assert!(validate(&m).is_err());
        m.trigger = 1;
        m.steps[0].duration_ms = 0;
        assert!(validate(&m).is_err());
        m.steps = vec![Step::default(); 201];
        assert!(validate(&m).is_err());
    }
}
