use async_std;
use async_std::task;
use color_eyre::eyre::{eyre, Result, WrapErr};
use color_eyre::section::PanicMessage;
use owo_colors::OwoColorize;
use semver::Version;
use std::fs;
use std::process::Command;
use std::{fmt, panic::Location};
use structopt::StructOpt;
use sys_info::{os_release, os_type};
use tracing::instrument;
use url::Url;

mod cli_args;
mod incremental;
mod toast;

use cli_args::Toast;
use incremental::{incremental_compile, IncrementalOpts};
use toast::breadbox::parse_import_map;

struct MyPanicMessage;

const VERSION: &'static str = env!("CARGO_PKG_VERSION");

impl PanicMessage for MyPanicMessage {
    fn display(&self, pi: &std::panic::PanicInfo<'_>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", "The application panicked (crashed).".red())?;

        // Print panic message.
        let payload = pi
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| pi.payload().downcast_ref::<&str>().cloned())
            .unwrap_or("<non string panic payload>");

        write!(f, "Message:  ")?;
        writeln!(f, "{}", payload.cyan())?;

        // If known, print panic location.
        write!(f, "Location: ")?;
        if let Some(loc) = pi.location() {
            write!(f, "{}", loc.file().purple())?;
            write!(f, ":")?;
            write!(f, "{}", loc.line().purple())?;

            write!(
                f,
                "\n\nConsider reporting the bug using this URL: {}",
                custom_url(loc, payload).cyan()
            )?;
        } else {
            write!(f, "<unknown>")?;
        }

        Ok(())
    }
}

fn custom_url(location: &Location<'_>, message: &str) -> impl fmt::Display {
    let url_result = Url::parse_with_params(
        "https://github.com/christopherBiscardi/toast/issues/new",
        &[
            ("title", "<autogenerated-issue>"),
            (
                "body",
                format!(
                    "## Metadata
|key|value|
|--|--|
|**version**|{}|
|**os_type**|{}|
|**os_release**|{}|
|**message**|{}|
|**location**|{}|
## More info
",
                    VERSION,
                    os_type().unwrap_or("unavailable".to_string()),
                    os_release().unwrap_or("unavailable".to_string()),
                    message,
                    location,
                )
                .as_str(),
            ),
        ],
    );
    match &url_result {
        Ok(url_struct) => format!("{}", url_struct),
        Err(_e) => String::from("https://github.com/christopherBiscardi/toast/issues/new"),
    }
}

fn get_npm_bin_dir() -> String {
    let output = Command::new("npm")
        .arg("bin")
        .output()
        .expect("failed to execute process");
    match String::from_utf8(output.stdout) {
        Ok(output_string) => output_string,
        Err(e) => {
            println!("utf8 conversion error {}", e);
            panic!("npm bin location could not be found, exiting")
        }
    }
}

fn check_node_version() -> Result<()> {
    let minimum_required_node_major_version = Version {
        major: 14,
        minor: 0,
        patch: 0,
        pre: vec![],
        build: vec![],
    };

    let mut cmd = Command::new("node");
    cmd.arg("-v");
    let output = cmd
        .output()
        .wrap_err_with(|| "Failed to execute `node -v` Command and collect output")?;
    let version_string = std::str::from_utf8(&output.stdout)
        .wrap_err_with(|| "Failed to create utf8 string from node -v Command output")?;
    let version_string_trimmed = version_string.trim_start_matches("v");
    let current_node_version_result = Version::parse(version_string_trimmed);
    match current_node_version_result {
        Ok(current_node_version) => {
            if current_node_version < minimum_required_node_major_version {
                Err(eyre!(format!(
                    "node version {} doesn't meet the minimum required version {}",
                    current_node_version, minimum_required_node_major_version
                )))
            } else {
                Ok(())
            }
        }
        Err(_e) => Err(eyre!(format!(
            "Couldn't parse node version from trimmed version `{}`, original string is `{}`",
            version_string_trimmed, version_string
        ))),
    }
}

#[instrument]
fn main() -> Result<()> {
    #[cfg(feature = "capture-spantrace")]
    install_tracing();

    color_eyre::config::HookBuilder::default()
        .panic_message(MyPanicMessage)
        .install()?;

    check_node_version()?;
    // let client = libhoney::init(libhoney::Config {
    //     options: libhoney::client::Options {
    //         api_key: "YOUR_API_KEY".to_string(),
    //         dataset: "honeycomb-rust-example".to_string(),
    //         ..libhoney::client::Options::default()
    //     },
    //     transmission_options: libhoney::transmission::Options::default(),
    // });
    // event := builder.new_event()
    // event.add_field("key", Value::String("val".to_string())), event.add(data)
    let npm_bin_dir = get_npm_bin_dir();
    let opt = Toast::from_args();
    // println!("{:?}", opt);
    match opt {
        Toast::Incremental {
            debug,
            input_dir,
            output_dir,
        } => {
            let import_map = {
                let import_map_filepath = input_dir.join("public/web_modules/import-map.json");
                let contents = fs::read_to_string(&import_map_filepath).wrap_err_with(|| {
                    format!(
                        "Failed to read `import-map.json` from `{}`",
                        &import_map_filepath.display()
                    )
                })?;
                parse_import_map(&contents).wrap_err_with(|| {
                    format!(
                        "Failed to parse import map from content `{}` at `{}`",
                        contents,
                        &import_map_filepath.display()
                    )
                })?
            };

            task::block_on(incremental_compile(IncrementalOpts {
                debug,
                project_root_dir: &input_dir,
                output_dir: match output_dir {
                    Some(v) => v,
                    None => {
                        let full_output_dir = input_dir.join("public");
                        std::fs::create_dir_all(&full_output_dir).wrap_err_with(|| {
                            format!(
                                "Failed create directories for path `{}`",
                                &full_output_dir.display()
                            )
                        })?;
                        full_output_dir
                            .canonicalize()
                            .wrap_err_with(|| {
                                format!("Failed canonicalize the output directory path")
                            })?
                            .to_path_buf()
                    }
                },
                npm_bin_dir,
                import_map,
            }))
        }
    }
    // println!("{}", result)
    // .expect("failed to process file");
    // event.send(&mut client)
    // client.close();
}

#[cfg(feature = "capture-spantrace")]
fn install_tracing() {
    use tracing_error::ErrorLayer;
    use tracing_subscriber::prelude::*;
    use tracing_subscriber::{fmt, EnvFilter};

    let fmt_layer = fmt::layer().with_target(false);
    let filter_layer = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new("info"))
        .unwrap();

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(fmt_layer)
        .with(ErrorLayer::default())
        .init();
}
