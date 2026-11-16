use crate::protocol::{Result, dword, word};
pub const SIZE: usize = 1592;
pub const ENABLED: u32 = 0x20200911;
pub const BUTTONS: [&str; 22] = [
    "A", "B", "X", "Y", "LB", "RB", "LT", "RT", "L3", "R3", "Select", "Start", "Star", "Home",
    "Up", "Down", "Left", "Right", "PR", "PL", "L4", "R4",
];
pub const KEYS: [(&str, u32); 35] = [
    ("None", 0),
    ("A", 0x2000),
    ("B", 0x1000),
    ("X", 0x10),
    ("Y", 0x20),
    ("LB", 0x400),
    ("RB", 0x800),
    ("LT", 0x4000),
    ("RT", 0x8000),
    ("L3", 2),
    ("R3", 4),
    ("Select", 8),
    ("Start", 1),
    ("Menu", 0x10000),
    ("Home", 0x20000),
    ("Up", 0x200),
    ("Down", 0x100),
    ("Left", 0x80),
    ("Right", 0x40),
    ("PR", 0x2000000),
    ("PL", 0x4000000),
    ("L4", 0x200000),
    ("R4", 0x40000000),
    ("P5", 0x100000),
    ("Screenshot", 0x400000),
    ("Turbo single", 0x800000),
    ("Turbo auto", 0x1000000),
    ("LS Up", 0x8000010),
    ("LS Down", 0x8000020),
    ("LS Left", 0x8000040),
    ("LS Right", 0x8000080),
    ("RS Up", 0x8000001),
    ("RS Down", 0x8000002),
    ("RS Left", 0x8000004),
    ("RS Right", 0x8000008),
];
pub const COLORWAYS: [(u8, &str); 3] = [
    (2, "WUCHANG: Fallen Feathers"),
    (3, "Honkai: Star Rail — Evernight (lilac)"),
    (4, "Clear Blue"),
];
pub fn colorway(edition: u8) -> String {
    COLORWAYS
        .iter()
        .find(|x| x.0 == edition)
        .map(|x| x.1.to_string())
        .unwrap_or_else(|| format!("Standard / unconfirmed edition {edition}"))
}
pub fn key_name(value: u32) -> String {
    if value == 0x8000008 {
        return "RS Right".into();
    }
    KEYS.iter()
        .find(|x| x.1 == value)
        .map(|x| x.0.to_string())
        .unwrap_or_else(|| format!("0x{value:08x}"))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config(pub Vec<u8>);
impl Config {
    pub fn parse(b: Vec<u8>) -> Result<Self> {
        if b.len() != SIZE {
            Err("Invalid configuration length".into())
        } else {
            Ok(Self(b))
        }
    }
    pub fn u32(&self, o: usize) -> u32 {
        dword(&self.0, o)
    }
    pub fn u16(&self, o: usize) -> u16 {
        word(&self.0, o)
    }
    pub fn set32(&mut self, o: usize, v: u32) {
        self.0[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn set16(&mut self, o: usize, v: u16) {
        self.0[o..o + 2].copy_from_slice(&v.to_le_bytes());
    }
    pub fn float(&self, o: usize) -> f32 {
        let v = f32::from_bits(self.u32(o));
        if v.is_finite() { v.clamp(0., 1.) } else { 1. }
    }
    pub fn set_float(&mut self, o: usize, v: f32) {
        self.set32(o, v.clamp(0., 1.).to_bits());
    }
    pub fn name(&self, slot: usize) -> String {
        let b = &self.0[20 + 32 * slot..52 + 32 * slot];
        String::from_utf16_lossy(
            &b.chunks_exact(2)
                .map(|b| u16::from_be_bytes([b[0], b[1]]))
                .take_while(|&c| c != 0 && c != 0xffff)
                .collect::<Vec<_>>(),
        )
    }
    pub fn set_name(&mut self, slot: usize, name: &str) {
        let o = 20 + slot * 32;
        self.0[o..o + 32].fill(0);
        for (i, c) in name.encode_utf16().take(16).enumerate() {
            self.0[o + 2 * i..o + 2 * i + 2].copy_from_slice(&c.to_be_bytes());
        }
    }
    pub fn init_slot(&mut self, s: usize) {
        self.set32(4 * s, ENABLED);
        self.set_name(s, &format!("Profile {}", s + 1));
        for o in [
            116 + 12 * s,
            152 + 8 * s,
            176 + 8 * s,
            200 + 8 * s,
            224 + 92 * s,
            500 + 216 * s,
            1148 + 8 * s,
            1208 + 12 * s,
            1244 + 16 * s,
            1292 + 100 * s,
        ] {
            self.set32(o, ENABLED);
        }
        self.set_float(120 + 12 * s, 1.);
        self.set_float(124 + 12 * s, 1.);
        self.0[156 + 8 * s..160 + 8 * s].copy_from_slice(&[17, 128, 17, 128]);
        self.0[180 + 8 * s..184 + 8 * s].copy_from_slice(&[0, 255, 0, 255]);
        self.set32(204 + 8 * s, 0);
        let defaults = [
            0x2000, 0x1000, 0x10, 0x20, 0x400, 0x800, 0x4000, 0x8000, 2, 4, 8, 1, 0x10000, 0x20000,
            0x200, 0x100, 0x80, 0x40, 0, 0, 0, 0,
        ];
        for (i, v) in defaults.iter().enumerate() {
            self.set32(228 + 92 * s + 4 * i, *v);
        }
        self.0[504 + 216 * s..716 + 216 * s].fill(0);
        self.0[1152 + 8 * s..1156 + 8 * s].copy_from_slice(&[1, 100, 1, 100]);
        self.set32(1172 + 12 * s, 0x20190000);
        self.set32(1176 + 12 * s, 0);
        self.0[1180 + 12 * s..1184 + 12 * s].copy_from_slice(&[1, 10, 40, 0]);
        self.set32(1212 + 12 * s, 0x260032);
        self.set32(1216 + 12 * s, 0x084e50);
        self.set32(1248 + 16 * s, 0xcc6200);
        self.set32(1252 + 16 * s, 0xcc0000);
        self.0[1256 + 16 * s] = 7;
        for i in 0..24 {
            self.set32(1296 + 100 * s + 4 * i, 0x7e3200);
        }
    }
    pub fn reset_section(&mut self, s: usize, section: usize) -> Result<()> {
        if s > 2 || !(1..=5).contains(&section) {
            return Err("Invalid settings section".into());
        }
        let mut defaults = self.clone();
        defaults.init_slot(s);
        let ranges = match section {
            1 => vec![(224 + 92 * s, 92)],
            2 => vec![(152 + 8 * s, 8), (176 + 8 * s, 8)],
            3 => vec![(116 + 12 * s, 12), (1148 + 8 * s, 8)],
            4 => vec![(1172 + 12 * s, 12)],
            _ => vec![
                (1208 + 12 * s, 12),
                (1244 + 16 * s, 16),
                (1292 + 100 * s, 100),
            ],
        };
        for (o, n) in ranges {
            self.0[o..o + n].copy_from_slice(&defaults.0[o..o + n]);
        }
        let special = 204 + 8 * s;
        if section == 2 {
            self.set32(special, self.u32(special) & !0x19f);
        }
        if section == 3 {
            self.set32(special, self.u32(special) & !0x8000);
        }
        Ok(())
    }
    pub fn enable_slot(&mut self, s: usize) {
        if self.u32(4 * s) == u32::MAX || self.u32(4 * s) == 0 {
            self.init_slot(s);
        } else {
            self.set32(4 * s, ENABLED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn section_defaults_preserve_other_settings() {
        let mut c = Config(vec![0; SIZE]);
        c.init_slot(0);
        c.set_name(0, "Keep me");
        c.0[156] = 70;
        c.set_float(120, 0.3);
        c.set32(204, 0x8001);
        let before = c.clone();
        c.reset_section(0, 2).unwrap();
        assert_eq!(c.0[156], 17);
        assert_eq!(c.name(0), "Keep me");
        assert_eq!(c.u32(204), 0x8000);
        assert_eq!(c.float(120), before.float(120));
        assert_eq!(&c.0[224..], &before.0[224..]);
        c.reset_section(0, 3).unwrap();
        assert_eq!(c.float(120), 1.);
        assert_eq!(c.u32(204), 0);
        assert!(c.reset_section(3, 1).is_err());
    }
    #[test]
    fn profile_preserves_others() {
        let mut c = Config::parse(vec![255; SIZE]).unwrap();
        c.init_slot(1);
        c.set_name(1, "Test 🎮");
        assert_eq!(c.name(1), "Test 🎮");
        assert_eq!(c.u32(4), ENABLED);
        assert_eq!(c.u32(0), u32::MAX);
        assert_eq!(c.u32(228 + 92), 0x2000);
        assert_eq!(c.0[152], 255);
        assert!(Config::parse(vec![]).is_err());
        c.set_float(120, 2.);
        assert_eq!(c.float(120), 1.);
        assert!(colorway(2).contains("WUCHANG"));
        assert!(colorway(255).contains("unconfirmed"));
        c.set32(4, 0x20190000);
        let before = c.clone();
        c.enable_slot(1);
        assert_eq!(c.0[8..], before.0[8..]);
        assert_eq!(c.u32(4), ENABLED);
    }
}
