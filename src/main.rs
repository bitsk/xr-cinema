mod app;
mod command;
mod control;
mod demos;
mod hands;
mod library;
mod media;
mod panel;
mod renderer;
mod xr;

use anyhow::{bail, ensure, Context, Result};
use app::App;
use command::{Command, Projection, Scene, Stereo};
use std::{env, ffi::OsString, io, path::PathBuf, thread, time::Duration};

const HELP: &str = "空间影院 · openKylin XR Cinema

xr-cinema [OPTIONS] [--] [VIDEO ...]
xr-cinema ctl '{\"op\":\"status\"}'

  --library DIR                 初始片库目录
  --demo                        打开内置示例短片「环行星」
  --list-demos                  列出所有演示片及投影、分眼格式
  --demo=ID                     播放指定演示片，自动应用片源格式
  --projection flat|180|360     选择平面或等距柱状投影
  --stereo mono|sbs|tb          单眼、左右或上下排列
  --squeezed                    半宽／半高立体片源
  --swap-eyes                   交换左右眼
  --scene cinema|void           影院或暗场
  --headless                    只运行媒体和控制接口
  -h, --help                    显示帮助

菜单键打开控制面板，扳机选择。运行时通过本地 Unix socket 接受 v1 JSON 指令。";

fn next(iter: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<OsString> {
    iter.next()
        .with_context(|| format!("{flag} requires a value"))
}

fn main() -> Result<()> {
    // HyperSpace can give Wayland clients a private runtime directory. The XR
    // compositor and the player's local control socket use the login session.
    if let Some(dir) = env::var_os("XR_OPENXR_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        env::set_var("XDG_RUNTIME_DIR", dir);
    }
    let mut args = env::args_os().skip(1).peekable();
    let socket = control::default_socket();
    if args.peek().is_some_and(|a| a == "ctl") {
        args.next();
        let arg = next(&mut args, "ctl")?;
        ensure!(args.next().is_none(), "pass one JSON command to ctl");
        let cmd = serde_json::from_str(arg.to_str().context("JSON must be UTF-8")?)?;
        println!("{}", control::send(&socket, cmd)?);
        return Ok(());
    }
    let mut dir = None;
    let mut files = Vec::<PathBuf>::new();
    let mut commands = Vec::new();
    let mut headless = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("-h" | "--help") => {
                println!("{HELP}");
                return Ok(());
            }
            Some("--") => {
                files.extend(args.map(PathBuf::from));
                break;
            }
            Some("--library") => dir = Some(PathBuf::from(next(&mut args, "--library")?)),
            Some("--headless") => headless = true,
            Some("--demo") => files.push(demos::by_id("rings")?.path()),
            Some("--list-demos") => {
                for d in demos::catalog() {
                    println!(
                        "{}\t{}\t{}\t{}",
                        d.id,
                        d.format(),
                        d.title,
                        d.path().display()
                    );
                }
                return Ok(());
            }
            Some(flag) if flag.starts_with("--demo=") => {
                files.push(demos::by_id(&flag[7..])?.path())
            }
            Some("--squeezed") => commands.push(Command::SetSqueezed { enabled: true }),
            Some("--swap-eyes") => commands.push(Command::SwapEyes { enabled: true }),
            Some("--projection") => commands.push(Command::SetProjection {
                projection: match next(&mut args, "--projection")?.to_str() {
                    Some("flat") => Projection::Flat,
                    Some("180") => Projection::Equirect180,
                    Some("360") => Projection::Equirect360,
                    _ => bail!("--projection expects flat, 180 or 360"),
                },
            }),
            Some("--stereo") => commands.push(Command::SetStereo {
                stereo: match next(&mut args, "--stereo")?.to_str() {
                    Some("mono") => Stereo::Mono,
                    Some("sbs") => Stereo::Sbs,
                    Some("tb") => Stereo::TopBottom,
                    _ => bail!("--stereo expects mono, sbs or tb"),
                },
            }),
            Some("--scene") => commands.push(Command::SetScene {
                scene: match next(&mut args, "--scene")?.to_str() {
                    Some("cinema") => Scene::Cinema,
                    Some("void") => Scene::Void,
                    _ => bail!("--scene expects cinema or void"),
                },
            }),
            Some(s) if s.starts_with('-') => bail!("unknown option {s}\n{HELP}"),
            _ => files.push(arg.into()),
        }
    }
    let server = match control::Server::start(&socket) {
        Ok(s) => s,
        Err(e)
            if e.downcast_ref::<io::Error>()
                .is_some_and(|err| err.kind() == io::ErrorKind::WouldBlock) =>
        {
            ensure!(
                !headless && dir.is_none() && files.len() <= 1,
                "player already running; use ctl, or quit before starting another playlist"
            );
            if let Some(file) = files.first() {
                control::send(
                    &socket,
                    Command::Open {
                        path: file.canonicalize()?,
                    },
                )?;
            }
            for cmd in commands {
                control::send(&socket, cmd)?;
            }
            control::send(&socket, Command::ShowPanel { visible: true })?;
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    let mut app = App::new(
        server,
        &dir.unwrap_or_else(library::default_directory),
        headless,
    )?;
    app.playlist(files)?;
    for cmd in commands {
        app.apply(cmd)?;
    }
    if headless {
        while !app.shutdown_requested() {
            app.tick()?;
            thread::sleep(Duration::from_millis(8));
        }
        Ok(())
    } else {
        xr::run(&mut app).context("OpenXR 播放会话")
    }
}
