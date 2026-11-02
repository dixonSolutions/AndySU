use std::path::{Path, PathBuf};

use andysu_cli::{keys, now, pack, publish, read, serve, write, R};
use andysu_core::manifest::PatchFormat;
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "andysu",
    version,
    about = "AndySU: complete, permission-free self-updates for apps outside the stores"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Fmt {
    Zstd,
    Bsdiff,
}

impl From<Fmt> for PatchFormat {
    fn from(f: Fmt) -> Self {
        match f {
            Fmt::Zstd => PatchFormat::Zstd,
            Fmt::Bsdiff => PatchFormat::Bsdiff,
        }
    }
}

#[derive(Args, Clone)]
struct SignArgs {
    /// Signing key (root key, or a release key together with --cert).
    #[arg(long, env = "ANDYSU_KEY_FILE")]
    key: PathBuf,
    /// Root-signed certificate for a release key.
    #[arg(long, env = "ANDYSU_CERT_FILE")]
    cert: Option<PathBuf>,
    /// Days until the manifest expires (clients reject expired manifests).
    #[arg(long, default_value_t = 30)]
    expires_days: i64,
}

impl SignArgs {
    fn signer(&self) -> R<publish::Signer> {
        Ok(publish::Signer {
            key: keys::load_secret(&self.key)?,
            cert: self.cert.as_deref().map(keys::load_cert).transpose()?,
        })
    }
}

#[derive(Subcommand)]
enum ClientCmd {
    /// Check, download (resuming, deltas first), verify and stage.
    Update {
        #[arg(long)]
        url: String,
        #[arg(long = "pub", required = true)]
        public: Vec<PathBuf>,
        #[arg(long)]
        app: String,
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long, default_value = "x86_64")]
        abi: Vec<String>,
        #[arg(long, default_value_t = 1)]
        host_api: u64,
        #[arg(long, default_value_t = 0)]
        api_level: u64,
    },
    /// Cold-start decision (promote pending, count trial boots, roll back crash loops).
    Launch {
        #[arg(long)]
        no_apply: bool,
        #[arg(long, default_value_t = 2)]
        max_attempts: u32,
    },
    /// The payload reached a healthy state: commit.
    Healthy,
    /// Init exception, crash or watchdog: roll back if in trial.
    Fail {
        #[arg(long, default_value = "reported failure")]
        reason: String,
    },
    /// Live swap: pending becomes current in trial without a restart.
    Live,
    State,
    Gc {
        #[arg(long)]
        low_storage: bool,
    },
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a root key pair (<name>-root.key, <name>-public.pem), or a certified release key.
    Keygen {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "andysu")]
        name: String,
        /// Create a short-lived release key certified by this root key instead.
        #[arg(long)]
        release_from: Option<PathBuf>,
        #[arg(long, default_value_t = 90)]
        days: i64,
    },
    /// Set up AndySU in a project: detect its type, generate keys, write andysu.toml and CI.
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        app: Option<String>,
        /// GitHub repository (owner/name) that hosts releases.
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        no_keys: bool,
    },
    /// From andysu.toml, write the host's config.json and andysu-host.json (what the host declares).
    HostConfig {
        #[arg(long, default_value = "andysu.toml")]
        toml: PathBuf,
        #[arg(long)]
        config_out: PathBuf,
        #[arg(long)]
        host_out: Option<PathBuf>,
    },
    /// Split a finished APK into a frozen host APK and a payload directory.
    Split(andysu_cli::split::SplitArgs),
    /// Turn a payload directory into a release inside a site directory.
    Pack {
        payload: PathBuf,
        #[arg(long)]
        site: PathBuf,
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long)]
        id: Option<String>,
        /// Frozen host declaration (andysu-host.json); the build fails if the payload needs more.
        #[arg(long)]
        host: Option<PathBuf>,
        /// Content path prefixes that can be applied hot (without a restart).
        #[arg(long)]
        hot: Vec<String>,
        /// Payload directories shipped as one zip component.
        #[arg(long)]
        archive: Vec<String>,
        /// Build deltas against this many recent releases of the channel.
        #[arg(long, default_value_t = 3)]
        deltas: usize,
        #[arg(long, value_enum, default_value_t = Fmt::Zstd)]
        delta_format: Fmt,
        /// Absolute URL prefix for new blobs (e.g. a GitHub release download URL).
        #[arg(long)]
        base_url: Option<String>,
        /// Fail instead of warn when a library is not 16 KB aligned.
        #[arg(long)]
        require_16k: bool,
        /// Allow host_api_min above the host's level (a tier-C shell update will be needed).
        #[arg(long)]
        allow_shell: bool,
        /// Also write the release as a ready slot directory (the payload built into a host APK).
        #[arg(long)]
        emit_slot: Option<PathBuf>,
    },
    /// Add a packed release to a channel manifest and sign it.
    Publish {
        #[arg(long)]
        site: PathBuf,
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long)]
        release: String,
        #[arg(long)]
        rollout: Option<u8>,
        #[arg(long)]
        mandatory: bool,
        #[arg(long)]
        notes_url: Option<String>,
        #[arg(long, default_value_t = 5)]
        keep: usize,
        #[command(flatten)]
        sign: SignArgs,
    },
    /// Re-sign a channel manifest (refresh expiry, change a rollout percentage).
    Sign {
        #[arg(long)]
        site: PathBuf,
        #[arg(long, default_value = "stable")]
        channel: String,
        /// RELEASE=PERCENT
        #[arg(long)]
        rollout: Option<String>,
        #[command(flatten)]
        sign: SignArgs,
    },
    /// Roll a channel back: republish a known-good release with a higher sequence.
    Rollback {
        #[arg(long)]
        site: PathBuf,
        #[arg(long, default_value = "stable")]
        channel: String,
        #[arg(long)]
        to: String,
        #[command(flatten)]
        sign: SignArgs,
    },
    /// Verify a manifest (path or URL) exactly as a client would.
    Verify {
        src: String,
        #[arg(long = "pub", required = true)]
        public: Vec<PathBuf>,
        #[arg(long)]
        app: Option<String>,
        #[arg(long)]
        channel: Option<String>,
        /// Also download and hash every blob and delta.
        #[arg(long)]
        blobs: bool,
    },
    /// Create a binary delta between two files.
    Diff {
        old: PathBuf,
        new: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = Fmt::Zstd)]
        format: Fmt,
    },
    /// Apply a binary delta.
    Patch {
        old: PathBuf,
        patch: PathBuf,
        #[arg(short, long)]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t = Fmt::Zstd)]
        format: Fmt,
        #[arg(long, default_value_t = 1 << 32)]
        max_size: u64,
    },
    /// Inspect an ELF shared library (SONAME, DT_NEEDED, ABI, alignment).
    Elf { file: PathBuf },
    /// A simulated client over a local state directory (also used by the desktop host).
    Client {
        #[arg(long)]
        root: PathBuf,
        #[command(subcommand)]
        cmd: ClientCmd,
    },
    /// Serve a directory over HTTP with Range and ETag support (local remotes and tests).
    Serve {
        dir: PathBuf,
        #[arg(long, default_value = "127.0.0.1:8787")]
        addr: String,
        /// Drop each path's first response after this many body bytes.
        #[arg(long)]
        drop_after: Option<u64>,
        /// Only drop paths with this prefix (e.g. blobs/).
        #[arg(long, default_value = "")]
        drop_prefix: String,
        /// Throttle response bodies to about this many KiB per second.
        #[arg(long)]
        throttle_kbps: Option<u64>,
    },
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap()
}

fn roots(paths: &[PathBuf]) -> R<Vec<andysu_core::keys::PublicKey>> {
    paths.iter().map(|p| keys::load_public(p)).collect()
}

fn core<T>(r: andysu_core::Result<T>) -> R<T> {
    r.map_err(|e| format!("{} ({})", e, e.code()))
}

fn run(cli: Cli) -> R<()> {
    match cli.cmd {
        Cmd::Keygen { dir, name, release_from: None, .. } => {
            let g = keys::keygen(&dir, &name)?;
            println!(
                "root key id {}\n  private: {}  (keep offline; never commit)\n  public:  {}  (compile into the host)",
                g.id,
                g.private.display(),
                g.public.display()
            );
        }
        Cmd::Keygen { dir, name, release_from: Some(root), days } => {
            let root = keys::load_secret(&root)?;
            let (k, c) = keys::release_key(&dir, &name, &root, now() - 300, days)?;
            println!("release key {} certified for {days} days\n  certificate: {}", k.display(), c.display());
        }
        Cmd::Init { dir, app, repo, no_keys } => {
            println!("{}", andysu_cli::init::init(&dir, app, repo, !no_keys)?);
        }
        Cmd::HostConfig { toml, config_out, host_out } => {
            let t = andysu_cli::init::AndySUToml::load(&toml)?;
            let dir = toml.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
            write(&config_out, json(&t.host_config(dir)?).as_bytes())?;
            if let Some(h) = host_out {
                write(&h, json(&t.host_spec()).as_bytes())?;
            }
            println!("manifest URL: {}", t.manifest_url()?);
        }
        Cmd::Split(a) => {
            println!("{}", json(&andysu_cli::split::split(&a)?));
        }
        Cmd::Pack {
            payload,
            site,
            channel,
            id,
            host,
            hot,
            archive,
            deltas,
            delta_format,
            base_url,
            require_16k,
            allow_shell,
            emit_slot,
        } => {
            let host = host
                .map(|h| serde_json::from_slice(&read(&h)?).map_err(|e| format!("{}: {e}", h.display())))
                .transpose()?;
            let o = pack::Options {
                payload,
                site,
                channel,
                id,
                host,
                hot,
                archive,
                deltas,
                delta_format: delta_format.into(),
                base_url,
                require_16k,
                allow_shell,
                emit_slot,
            };
            let rep = pack::pack(&o)?;
            for w in &rep.warnings {
                eprintln!("warning: {w}");
            }
            println!("{}", json(&rep));
        }
        Cmd::Publish { site, channel, release, rollout, mandatory, notes_url, keep, sign } => {
            let o = publish::PublishOptions {
                channel,
                release,
                rollout,
                mandatory,
                notes_url,
                keep,
                expires_days: sign.expires_days,
                now: now(),
            };
            let m = publish::publish(&site, &o, &sign.signer()?)?;
            println!(
                "published {} to {} (manifest sequence {}, expires {})",
                o.release, m.channel, m.sequence, m.expires
            );
        }
        Cmd::Sign { site, channel, rollout, sign } => {
            let ro = match &rollout {
                Some(s) => {
                    let (id, p) = s.split_once('=').ok_or("--rollout wants RELEASE=PERCENT")?;
                    Some((id, p.parse::<u8>().map_err(|e| e.to_string())?))
                }
                None => None,
            };
            let m = publish::refresh(&site, &channel, &sign.signer()?, now(), sign.expires_days, ro)?;
            println!("signed {} sequence {} expires {}", m.channel, m.sequence, m.expires);
        }
        Cmd::Rollback { site, channel, to, sign } => {
            let (m, id) = publish::rollback(&site, &channel, &to, &sign.signer()?, now(), sign.expires_days)?;
            println!("channel {} now serves {id} (content of {to}), manifest sequence {}", m.channel, m.sequence);
        }
        Cmd::Verify { src, public, app, channel, blobs } => {
            print!("{}", publish::verify(&src, &roots(&public)?, app.as_deref(), channel.as_deref(), blobs, now())?);
        }
        Cmd::Diff { old, new, out, format } => {
            let p = core(andysu_core::patch::create(format.into(), &read(&old)?, &read(&new)?))?;
            write(&out, &p)?;
            println!("{} bytes", p.len());
        }
        Cmd::Patch { old, patch, out, format, max_size } => {
            write(&out, &core(andysu_core::patch::apply(format.into(), &read(&old)?, &read(&patch)?, max_size))?)?;
        }
        Cmd::Elf { file } => {
            let i = andysu_cli::elf::info(&read(&file)?)?;
            println!(
                "{}",
                json(
                    &serde_json::json!({"abi": i.abi(), "soname": i.soname, "needed": i.needed, "min_load_align": i.min_load_align(), "aligned_16k": i.min_load_align() >= 16384})
                )
            );
        }
        Cmd::Client { root, cmd } => client(&root, cmd)?,
        Cmd::Serve { dir, addr, drop_after, drop_prefix, throttle_kbps } => {
            let faults = serve::Faults {
                drop_after,
                drop_prefix,
                throttle_bps: throttle_kbps.map(|k| k * 1024),
                ..Default::default()
            };
            let s = serve::spawn(&dir, &addr, faults).map_err(|e| e.to_string())?;
            eprintln!("serving {} on {}", dir.display(), s.url());
            let mut seen = 0;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let r = s.requests();
                for l in &r[seen..] {
                    eprintln!("{l}");
                }
                seen = r.len();
            }
        }
    }
    Ok(())
}

fn client(root: &Path, cmd: ClientCmd) -> R<()> {
    use andysu_core::ffi::api;
    let root_s = root.to_str().ok_or("root path is not UTF-8")?;
    let out = match cmd {
        ClientCmd::Update { url, public, app, channel, abi, host_api, api_level } => {
            let roots: Vec<String> =
                public.iter().map(|p| std::fs::read_to_string(p).map_err(|e| e.to_string())).collect::<R<_>>()?;
            let cfg = andysu_core::session::Config {
                app,
                channel,
                roots,
                device: andysu_core::select::Device { host_api, shell_version: 0, api_level, abis: abi },
                low_storage: false,
            };
            let store = core(andysu_core::store::Store::open(root))?;
            serde_json::to_value(core(andysu_core::transport::update_once(
                store,
                cfg,
                &url,
                andysu_cli::random(),
                now(),
            ))?)
            .unwrap()
        }
        ClientCmd::Launch { no_apply, max_attempts } => {
            core(api::launch(root_s, &andysu_cli::random::<16>(), now(), !no_apply, max_attempts))?
        }
        ClientCmd::Healthy => core(api::mark_healthy(root_s))?,
        ClientCmd::Fail { reason } => core(api::report_failure(root_s, &reason))?,
        ClientCmd::Live => core(api::begin_live(root_s, now()))?,
        ClientCmd::State => core(api::state(root_s))?,
        ClientCmd::Gc { low_storage } => core(api::gc(root_s, low_storage, &[]))?,
    };
    println!("{}", json(&out));
    Ok(())
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
