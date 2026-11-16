//! Canonical physical inputs plus quaternion-based six-axis orientation.
use crate::{
    device::{Link, nodes},
    protocol::{Packet, Transport, dword},
};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub const BUTTON_BITS: [u32; 22] = [
    0x2000, 0x1000, 0x10, 0x20, 0x400, 0x800, 0x4000, 0x8000, 2, 4, 8, 1, 0x10000, 0x20000, 0x200,
    0x100, 0x80, 0x40, 0x2000000, 0x4000000, 0x200000, 0x40000000,
];
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub gyro: [f32; 3],
    pub accel: [f32; 3],
    pub sticks: [f32; 4],
    pub triggers: [f32; 2],
    pub buttons: u32,
}
pub fn physical(b: &[u8]) -> Option<Sample> {
    if b.len() < 10 {
        return None;
    }
    Some(Sample {
        buttons: dword(b, 0),
        sticks: std::array::from_fn(|i| (b[4 + i] as f32 - 127.5) / 127.5),
        triggers: [b[9] as f32 / 255., b[8] as f32 / 255.],
        ..Default::default()
    })
}
pub fn parse(b: &[u8]) -> Option<Sample> {
    if b.len() < 27 || b[0] != 1 {
        return None;
    }
    let v: [f32; 6] =
        std::array::from_fn(|i| i16::from_le_bytes([b[15 + i * 2], b[16 + i * 2]]) as f32);
    let mut keys = 0;
    for (byte, mask, key) in [
        (8, 1, 0x2000),
        (8, 2, 0x1000),
        (8, 8, 0x10),
        (8, 16, 0x20),
        (8, 64, 0x400),
        (8, 128, 0x800),
        (8, 32, 0x4000000),
        (8, 4, 0x2000000),
        (9, 4, 8),
        (9, 8, 1),
        (9, 16, 0x20000),
        (9, 32, 2),
        (9, 64, 4),
        (9, 128, 0x10000),
        (10, 1, 0x200000),
        (10, 2, 0x40000000),
    ] {
        if b[byte] & mask != 0 {
            keys |= key;
        }
    }
    match b[1] & 15 {
        0 => keys |= 0x200,
        1 => keys |= 0x200 | 0x40,
        2 => keys |= 0x40,
        3 => keys |= 0x100 | 0x40,
        4 => keys |= 0x100,
        5 => keys |= 0x100 | 0x80,
        6 => keys |= 0x80,
        7 => keys |= 0x200 | 0x80,
        _ => {}
    }
    if b[7] > 5 {
        keys |= 0x4000;
    }
    if b[6] > 5 {
        keys |= 0x8000;
    }
    Some(Sample {
        gyro: [
            -v[4] * 2000. / 32767.,
            v[5] * 2000. / 32767.,
            -v[3] * 2000. / 32767.,
        ],
        accel: [v[0] / 4096., v[1] / 4096., v[2] / 4096.],
        sticks: std::array::from_fn(|i| (b[2 + i] as f32 - 127.5) / 127.5),
        triggers: [b[7] as f32 / 255., b[6] as f32 / 255.],
        buttons: keys,
    })
}
#[derive(Clone, Copy, Debug)]
pub struct Quaternion(pub [f32; 4]);
impl Default for Quaternion {
    fn default() -> Self {
        Self([1., 0., 0., 0.])
    }
}
impl Quaternion {
    pub fn conjugate(self) -> Self {
        let [w, x, y, z] = self.0;
        Self([w, -x, -y, -z])
    }
    pub fn mul(self, r: Self) -> Self {
        let [a, b, c, d] = self.0;
        let [e, f, g, h] = r.0;
        Self([
            a * e - b * f - c * g - d * h,
            a * f + b * e + c * h - d * g,
            a * g - b * h + c * e + d * f,
            a * h + b * g - c * f + d * e,
        ])
    }
    pub fn normalized(self) -> Self {
        let n = self.0.iter().map(|v| v * v).sum::<f32>().sqrt();
        if n < 0.0001 {
            Self::default()
        } else {
            Self(self.0.map(|v| v / n))
        }
    }
    pub fn rotate(self, p: [f32; 3]) -> [f32; 3] {
        let q = self
            .mul(Self([0., p[0], p[1], p[2]]))
            .mul(self.conjugate())
            .0;
        [q[1], q[2], q[3]]
    }
    pub fn angles(self) -> [f32; 3] {
        let [w, x, y, z] = self.0;
        [
            (2. * (w * x + y * z))
                .atan2(1. - 2. * (x * x + y * y))
                .to_degrees(),
            (2. * (w * z + x * y))
                .atan2(1. - 2. * (y * y + z * z))
                .to_degrees(),
            -(2. * (w * y - z * x)).clamp(-1., 1.).asin().to_degrees(),
        ]
    }
}
#[derive(Clone, Debug)]
pub struct View {
    pub sample: Sample,
    pub orientation: Quaternion,
    pub angles: [f32; 3],
    pub hz: f32,
    pub status: String,
    pub calibration: String,
    pub calibrate: bool,
    pub samples: u64,
    pub has_imu: bool,
    pub mode: &'static str,
    pub connection: &'static str,
    pub path: String,
    pub generation: u64,
    pub rumble: [f32; 2],
    last_control: Instant,
}
impl Default for View {
    fn default() -> Self {
        Self {
            sample: Sample::default(),
            orientation: Quaternion::default(),
            angles: [0.; 3],
            hz: 0.,
            status: "Connect the controller".into(),
            calibration: "Automatic still calibration".into(),
            calibrate: false,
            samples: 0,
            has_imu: false,
            mode: "Disconnected",
            connection: "",
            path: String::new(),
            generation: 0,
            rumble: [0.; 2],
            last_control: Instant::now() - Duration::from_secs(1),
        }
    }
}
#[derive(Default)]
pub struct Fusion {
    q: Quaternion,
    neutral: Quaternion,
    bias: [f32; 3],
    still: Vec<[f32; 3]>,
    calibrating: bool,
    initialized: bool,
}
impl Fusion {
    pub fn calibrate(&mut self) {
        self.still.clear();
        self.calibrating = true;
    }
    pub fn orientation(&self) -> Quaternion {
        self.neutral.conjugate().mul(self.q).normalized()
    }
    pub fn update(&mut self, s: &Sample, dt: f32) -> Option<&'static str> {
        let dt = dt.clamp(0.0001, 0.02);
        let measured = [-s.accel[1], s.accel[0], s.accel[2]];
        let norm = measured.iter().map(|v| v * v).sum::<f32>().sqrt();
        let gravity_ok = (0.85..1.15).contains(&norm);
        let mut result = None;
        if !self.initialized && gravity_ok {
            let pitch =
                s.accel[0].atan2((s.accel[1] * s.accel[1] + s.accel[2] * s.accel[2]).sqrt()) / 2.;
            let roll = s.accel[1].atan2(s.accel[2]) / 2.;
            self.q = Quaternion([pitch.cos(), pitch.sin(), 0., 0.]).mul(Quaternion([
                roll.cos(),
                0.,
                roll.sin(),
                0.,
            ]));
            self.neutral = self.q;
            self.initialized = true;
        }
        let quiet = s
            .gyro
            .iter()
            .zip(self.bias)
            .all(|(v, b)| (v - b).abs() < 1.5);
        if gravity_ok && (quiet || self.calibrating) {
            self.still.push(s.gyro);
            if self.still.len() >= 500 {
                let mean: [f32; 3] = std::array::from_fn(|i| {
                    self.still.iter().map(|v| v[i]).sum::<f32>() / self.still.len() as f32
                });
                let stable = self
                    .still
                    .iter()
                    .all(|v| (0..3).all(|i| (v[i] - mean[i]).abs() < 0.35));
                if stable {
                    self.bias = mean;
                    if self.calibrating {
                        self.neutral = self.q;
                    }
                    result = Some("Calibrated · automatic drift correction");
                } else if self.calibrating {
                    result = Some("Controller moved; hold it still");
                }
                self.still.clear();
                self.calibrating = false;
            }
        } else {
            self.still.clear();
        }
        let corrected: [f32; 3] = std::array::from_fn(|i| {
            let g = s.gyro[i] - self.bias[i];
            if g.abs() < 0.18 { 0. } else { g.to_radians() }
        });
        let mut omega = [corrected[0], -corrected[2], corrected[1]];
        if gravity_ok {
            let a = measured.map(|v| v / norm);
            let g = self.q.conjugate().rotate([0., 0., 1.]);
            let e = [
                a[1] * g[2] - a[2] * g[1],
                a[2] * g[0] - a[0] * g[2],
                a[0] * g[1] - a[1] * g[0],
            ];
            for i in 0..3 {
                omega[i] += e[i] * 2.;
            }
        }
        let d = self.q.mul(Quaternion([0., omega[0], omega[1], omega[2]])).0;
        self.q = Quaternion(std::array::from_fn(|i| self.q.0[i] + 0.5 * d[i] * dt)).normalized();
        result
    }
}
fn copy_controls(v: &mut View, s: &Sample) {
    v.sample.buttons = s.buttons;
    v.sample.sticks = s.sticks;
    v.sample.triggers = s.triggers;
}
fn input_link() -> Option<(Link, bool)> {
    let mut radio = None;
    for n in nodes()
        .ok()?
        .iter()
        .filter(|n| n.config && ![0x3206, 0x3208, 0x6013, 0x3107].contains(&n.pid))
    {
        let l = Link::open(n).ok()?;
        if crate::firmware::identify(&l).ok() == Some(0x6012) {
            return Some((l, true));
        }
        radio = Some((l, false));
    }
    radio
}
fn repaint_due(pending: &mut bool, changed: bool, animate: bool, elapsed: Duration) -> bool {
    *pending |= changed;
    let due = (*pending && elapsed >= Duration::from_millis(16))
        || (animate && elapsed >= Duration::from_millis(33));
    if due {
        *pending = false;
    }
    due
}
pub fn start(
    active: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    view: Arc<Mutex<View>>,
    ctx: eframe::egui::Context,
    animate: Arc<AtomicBool>,
) {
    let generation = Arc::new(AtomicU64::new(0));
    let watch_generation = generation.clone();
    let watch_stop = stop.clone();
    let watch_state = view.clone();
    let watch_ctx = ctx.clone();
    thread::spawn(move || {
        let signature = || {
            let mut paths = std::fs::read_dir("/sys/class/hidraw")
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| e.path().canonicalize().ok())
                .collect::<Vec<_>>();
            paths.sort();
            paths
        };
        let mut last = signature();
        while !watch_stop.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(500));
            let now = signature();
            if now != last {
                let new_generation = watch_generation.fetch_add(1, Ordering::Relaxed) + 1;
                watch_state.lock().unwrap().generation = new_generation;
                watch_ctx.request_repaint();
                last = now;
            }
        }
    });
    let a = active.clone();
    let end = stop.clone();
    let state = view.clone();
    let input_ctx = ctx.clone();
    thread::spawn(move || {
        while !end.load(Ordering::Relaxed) {
            if !a.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            if let Some((l, wired)) = input_link() {
                let seen = generation.load(Ordering::Relaxed);
                while generation.load(Ordering::Relaxed) == seen
                    && a.load(Ordering::Relaxed)
                    && !end.load(Ordering::Relaxed)
                {
                    let mut p = Packet::new(4, 3);
                    p.sub = 0x123;
                    let Ok(bytes) = l.request(p, Duration::from_millis(200)) else {
                        break;
                    };
                    if let Some(s) = physical(&bytes) {
                        let mut v = state.lock().unwrap();
                        let changed = v.sample.buttons != s.buttons
                            || v.sample.sticks != s.sticks
                            || v.sample.triggers != s.triggers
                            || v.path != l.node.path.to_string_lossy();
                        copy_controls(&mut v, &s);
                        v.last_control = Instant::now();
                        v.mode = if l.node.pid == 0x6012 {
                            "DInput"
                        } else {
                            "XInput"
                        };
                        v.connection = if wired { "USB" } else { "2.4 GHz" };
                        v.path = l.node.path.to_string_lossy().into_owned();
                        drop(v);
                        if changed {
                            input_ctx.request_repaint();
                        }
                    }
                    thread::sleep(Duration::from_millis(16));
                }
            } else {
                let mut v = state.lock().unwrap();
                v.path.clear();
                if !v.has_imu {
                    v.mode = "Disconnected";
                    v.connection = "";
                    copy_controls(&mut v, &Sample::default());
                }
            }
            {
                let mut v = state.lock().unwrap();
                if v.last_control.elapsed() > Duration::from_millis(500)
                    && !v.has_imu
                    && v.mode != "Disconnected"
                {
                    v.path.clear();
                    v.mode = "Disconnected";
                    v.connection = "";
                    copy_controls(&mut v, &Sample::default());
                    input_ctx.request_repaint();
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
    });
    thread::spawn(move || {
        let mut fusion = Fusion::default();
        while !stop.load(Ordering::Relaxed) {
            if !active.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                continue;
            }
            let link = nodes()
                .ok()
                .and_then(|ns| ns.into_iter().find(|n| n.motion))
                .and_then(|n| Link::open(&n).ok());
            let Some(link) = link else {
                let mut v = view.lock().unwrap();
                let changed = v.has_imu;
                v.has_imu = false;
                v.status = "Raw motion requires DInput on the receiver".into();
                drop(v);
                if changed {
                    ctx.request_repaint();
                }
                thread::sleep(Duration::from_millis(500));
                continue;
            };
            let mut last = Instant::now();
            let mut window = last;
            let mut count = 0;
            let mut last_paint = Instant::now() - Duration::from_secs(1);
            let mut last_report = Instant::now();
            let mut input_dirty = false;
            while active.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                let b = match link.read(100) {
                    Ok(b) => b,
                    Err(_) => break,
                };
                let Some(s) = parse(&b) else {
                    if last_report.elapsed() > Duration::from_millis(500) {
                        break;
                    }
                    continue;
                };
                let now = Instant::now();
                last_report = now;
                let dt = (now - last).as_secs_f32();
                last = now;
                count += 1;
                let mut v = view.lock().unwrap();
                let first = !v.has_imu;
                let changed = v.sample.buttons != s.buttons
                    || v.sample.sticks != s.sticks
                    || v.sample.triggers != s.triggers;
                if v.calibrate {
                    fusion.calibrate();
                    v.calibrate = false;
                    v.calibration = "Hold still…".into();
                }
                if let Some(text) = fusion.update(&s, dt) {
                    v.calibration = text.into();
                }
                v.sample.gyro = s.gyro;
                v.sample.accel = s.accel;
                if v.last_control.elapsed() > Duration::from_millis(250) {
                    copy_controls(&mut v, &s);
                    v.mode = "DInput";
                    v.connection = "2.4 GHz";
                }
                v.orientation = fusion.orientation();
                v.angles = v.orientation.angles();
                v.has_imu = true;
                v.samples += 1;
                v.status = "Live raw DInput motion".into();
                let elapsed = (now - window).as_secs_f32();
                if elapsed >= 1. {
                    v.hz = count as f32 / elapsed;
                    count = 0;
                    window = now;
                }
                drop(v);
                if first
                    || repaint_due(
                        &mut input_dirty,
                        changed,
                        animate.load(Ordering::Relaxed),
                        last_paint.elapsed(),
                    )
                {
                    ctx.request_repaint();
                    last_paint = now;
                }
            }
            let mut v = view.lock().unwrap();
            v.has_imu = false;
            if v.last_control.elapsed() > Duration::from_millis(500) {
                v.mode = "Disconnected";
                v.connection = "";
                copy_controls(&mut v, &Sample::default());
            }
            drop(v);
            ctx.request_repaint();
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_trigger_value_and_release_are_not_lost_to_throttling() {
        let mut pending = false;
        assert!(!repaint_due(
            &mut pending,
            true,
            false,
            Duration::from_millis(5)
        ));
        assert!(repaint_due(
            &mut pending,
            false,
            false,
            Duration::from_millis(17)
        ));
        assert!(!pending);
        assert!(!repaint_due(
            &mut pending,
            true,
            false,
            Duration::from_millis(3)
        ));
        assert!(repaint_due(
            &mut pending,
            false,
            false,
            Duration::from_millis(16)
        ));
    }
    #[test]
    #[ignore = "requires a connected Ultimate 2; read-only"]
    fn physical_monitor_hardware() {
        let active = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        let view = Arc::new(Mutex::new(View::default()));
        start(
            active,
            stop.clone(),
            view.clone(),
            eframe::egui::Context::default(),
            Arc::new(AtomicBool::new(false)),
        );
        thread::sleep(Duration::from_secs(2));
        let v = view.lock().unwrap().clone();
        stop.store(true, Ordering::Relaxed);
        println!(
            "mode={} connection={} buttons={:08x} sticks={:?} triggers={:?}",
            v.mode, v.connection, v.sample.buttons, v.sample.sticks, v.sample.triggers
        );
        assert_ne!(v.mode, "Disconnected");
        assert!(!v.path.is_empty() || v.has_imu);
    }
    #[test]
    fn captured_wired_inputs_and_dinput_mapping() {
        let b = [0, 32, 0, 0, 128, 127, 128, 127, 10, 200, 0, 0];
        let p = physical(&b).unwrap();
        assert_eq!(p.buttons, 0x2000);
        assert!((p.triggers[0] - 200. / 255.).abs() < 0.001);
        assert!(physical(&b[..8]).is_none());
        let mut raw = [0; 34];
        raw[0] = 1;
        raw[1] = 1;
        raw[8] = 0xe7;
        raw[9] = 0xfc;
        raw[10] = 3;
        raw[7] = 200;
        raw[19..21].copy_from_slice(&4096i16.to_le_bytes());
        let s = parse(&raw).unwrap();
        assert_eq!(s.accel, [0., 0., 1.]);
        assert!(s.buttons & 0x40000000 != 0);
        assert!(s.buttons & 0x2000000 != 0);
        assert!(s.buttons & 0x4000000 != 0);
        assert!(s.buttons & 0x200 != 0);
        assert!(s.buttons & 0x40 != 0);
        assert!(s.triggers[0] > 0.7);
    }
    #[test]
    fn orientation_stays_normalized_and_still_drift_is_removed() {
        let mut f = Fusion::default();
        let still = Sample {
            gyro: [0.3, 0.5, -0.2],
            accel: [0., 0., 1.],
            ..Default::default()
        };
        for _ in 0..1000 {
            f.update(&still, 0.001);
        }
        let before = f.orientation().angles();
        for _ in 0..10000 {
            f.update(&still, 0.001);
        }
        assert!((f.orientation().angles()[1] - before[1]).abs() < 0.02);
        assert!((f.q.0.iter().map(|v| v * v).sum::<f32>() - 1.).abs() < 0.0001);
    }
    #[test]
    fn gravity_correction_uses_the_same_axes_as_the_gyroscope() {
        let h = 20f32.to_radians() / 2.;
        for target in [
            Quaternion([h.cos(), h.sin(), 0., 0.]),
            Quaternion([h.cos(), 0., h.sin(), 0.]),
        ] {
            let g = target.conjugate().rotate([0., 0., 1.]);
            let sample = Sample {
                accel: [g[1], -g[0], g[2]],
                ..Default::default()
            };
            let mut fusion = Fusion {
                initialized: true,
                ..Default::default()
            };
            for _ in 0..3000 {
                fusion.update(&sample, 0.001);
            }
            let got = fusion.q.conjugate().rotate([0., 0., 1.]);
            assert!((0..3).all(|i| (got[i] - g[i]).abs() < 0.01));
            for _ in 0..3000 {
                fusion.update(
                    &Sample {
                        accel: [0., 0., 1.],
                        ..Default::default()
                    },
                    0.001,
                );
            }
            let level = fusion.q.conjugate().rotate([0., 0., 1.]);
            assert!(level[0].abs() < 0.01 && level[1].abs() < 0.01);
        }
    }
    #[test]
    fn quaternion_wheel_rotation_and_recenter() {
        let h = 45f32.to_radians();
        let q = Quaternion([h.cos(), 0., h.sin(), 0.]);
        let p = q.rotate([1., 0., 0.]);
        assert!(p[0].abs() < 0.001 && (p[2] + 1.).abs() < 0.001);
        let mut f = Fusion::default();
        let s = Sample {
            accel: [0., 0., 1.],
            ..Default::default()
        };
        f.update(&s, 0.001);
        f.q = q;
        f.neutral = q;
        assert!(f.orientation().angles().iter().all(|v| v.abs() < 0.01));
    }
    #[test]
    fn moving_calibration_rejected() {
        let mut f = Fusion::default();
        f.calibrate();
        let mut result = None;
        for n in 0..500 {
            result = f.update(
                &Sample {
                    gyro: [(n % 2) as f32 * 100.; 3],
                    accel: [0., 0., 1.],
                    ..Default::default()
                },
                0.001,
            );
        }
        assert!(result.unwrap().contains("moved"));
    }
}
