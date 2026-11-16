mod app;
mod artwork;
mod backup;
mod config;
mod device;
mod drawing;
mod firmware;
mod macros;
mod motion;
mod protocol;
use protocol::{Result, Transport};
use winit::platform::wayland::EventLoopBuilderExtWayland;
fn cli(args: &[String]) -> Result<bool> {
    match args.get(1).map(String::as_str) {
        Some("--probe-boot") => {
            let target = match args.get(2).map(String::as_str) {
                Some("controller") => firmware::Target::Controller,
                Some("dongle") => firmware::Target::Receiver,
                _ => return Err("Use --probe-boot controller|dongle".into()),
            };
            let before = device::nodes()?
                .into_iter()
                .filter(|n| n.config && ![0x3206, 0x3208].contains(&n.pid))
                .find_map(|n| {
                    let l = device::Link::open(&n).ok()?;
                    if firmware::identify(&l).ok()? != target.pid() {
                        return None;
                    }
                    let r = l.raw_query(&[0x81, 5, 0, 0x21, 1], &[2, 0x22]).ok()?;
                    (r.len() >= 4).then(|| protocol::word(&r, 2) as u32)
                });
            if target == firmware::Target::Controller && before.is_some() {
                backup::automatic(&device::Link::connect()?)?;
            }
            let link = firmware::boot(target)?;
            let pid = firmware::identify(&link)?;
            println!(
                "Bootloader target {:04x} verified. No erase/write commands sent.",
                pid
            );
            link.write(&protocol::Packet::new(5, 7).encode()?)?;
            drop(link);
            std::thread::sleep(std::time::Duration::from_secs(1));
            if let Some(version) = before {
                firmware::verify(target, version)?;
            }
            println!("Application firmware reconnected and verified");
            Ok(true)
        }
        Some("--probe-macros") => {
            let l = device::Link::connect()?;
            let c = l.read_config()?;
            let mut p = protocol::Packet::new(4, 0x102);
            p.sub = c.u16(16) << 8;
            p.length = 20;
            p.total = 20;
            let r = l.request(p, std::time::Duration::from_secs(1))?;
            println!(
                "Read {} bytes from macro storage: {:02x?}; no writes",
                r.len(),
                r
            );
            Ok(true)
        }
        Some("--list") => {
            for n in device::nodes()? {
                println!(
                    "{} {:04x} config={} motion={} {}",
                    n.path.to_string_lossy(),
                    n.pid,
                    n.config,
                    n.motion,
                    n.name
                );
            }
            Ok(true)
        }
        Some("--info") => {
            let l = device::Link::connect()?;
            let info = l.info()?;
            println!(
                "Firmware {}.{:02}\nEdition {}: {}\nDevice {}\nTheme {}\nFirmware target {:?}",
                info.firmware / 100,
                info.firmware % 100,
                info.edition,
                config::colorway(info.edition),
                info.path,
                l.theme()?,
                firmware::identify(&l)
            );
            Ok(true)
        }
        Some("--backup") => {
            let path = args
                .get(2)
                .ok_or("Usage: ultimate2-native --backup FILE.json")?;
            backup::Backup::capture(&device::Link::connect()?)?
                .write(std::path::Path::new(path))?;
            println!("Backup saved: {path}");
            Ok(true)
        }
        Some("--check-updates") => {
            for target in [firmware::Target::Controller, firmware::Target::Receiver] {
                for r in firmware::releases(target)? {
                    println!("{} {}: {}", target.name(), r.version, r.readme_en);
                }
            }
            Ok(true)
        }
        Some("--verify-downloads") => {
            for target in [firmware::Target::Controller, firmware::Target::Receiver] {
                let r = firmware::releases(target)?
                    .into_iter()
                    .next()
                    .ok_or("No firmware listed")?;
                let im = firmware::download(&r, target)?;
                println!(
                    "{}: version {}, {} bytes, target {:04x}, flash address {:x}; no hardware writes",
                    target.name(),
                    im.version,
                    im.bytes.len(),
                    target.pid(),
                    im.address
                );
            }
            Ok(true)
        }
        Some("--help") => {
            println!(
                "ultimate2-native [--list | --info | --backup FILE.json | --check-updates | --verify-downloads]\nWithout arguments, open the native Wayland configurator."
            );
            Ok(true)
        }
        _ => Ok(false),
    }
}
fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    match cli(&args) {
        Ok(true) => return Ok(()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
        _ => {}
    }
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("A native Wayland session is required.");
        std::process::exit(1);
    }
    let size = [1120., 820.];
    #[cfg(feature = "screenshots")]
    let size = std::env::var("U2_WINDOW_SIZE")
        .ok()
        .and_then(|s| {
            s.split_once('x')
                .map(|(w, h)| [w.parse().unwrap_or(1120.), h.parse().unwrap_or(820.)])
        })
        .unwrap_or(size);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size(size)
            .with_transparent(false)
            .with_min_inner_size([560., 520.])
            .with_app_id("io.github.sersium.Ultimate2"),
        renderer: eframe::Renderer::Glow,
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_wayland();
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Ultimate 2 — Native 0.2.0",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(&cc.egui_ctx)))),
    )
}
