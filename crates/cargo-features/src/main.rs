use std::{cmp::Ordering, fmt::Display, fs, process};

use color_eyre::Result;
use colored::Colorize;

use cargo_metadata::{Dependency, Metadata, MetadataCommand, Package};
use clap::{Args, Parser, Subcommand};
use nucleo_matcher::{Config, Matcher, pattern::Atom};
use toml_edit::DocumentMut;

#[derive(Parser)]
#[command(name = "cargo")]
#[command(bin_name = "cargo")]
#[command(arg_required_else_help = true)]
#[command(version, about = None, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Features(FeatuerArgs),
}

#[derive(Args)]
struct FeatuerArgs {
    /// Workspace package name
    #[arg(short, long, value_name = "PACKAGE_NAME")]
    package: Option<String>,

    /// Dependency name
    dependency: Option<String>,
}

struct Feature {
    name: String,
    includes: Vec<String>,
}

impl Display for Feature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}{}",
            self.name.blue(),
            format!(" = [{}]", self.includes.join(", ")).bright_black()
        )
    }
}

fn fuzzy_match<T: AsRef<str>>(items: impl IntoIterator<Item = T>, keyword: &str) -> Vec<(T, u16)> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let matches = Atom::new(
        keyword,
        nucleo_matcher::pattern::CaseMatching::Ignore,
        nucleo_matcher::pattern::Normalization::Smart,
        nucleo_matcher::pattern::AtomKind::Fuzzy,
        false,
    )
    .match_list(items, &mut matcher);

    matches
}

fn handle_prompt_result<T>(
    result: inquire::error::InquireResult<T>,
) -> inquire::error::InquireResult<T> {
    match result {
        Ok(result) => Ok(result),
        Err(err) => match err {
            inquire::InquireError::OperationInterrupted
            | inquire::InquireError::OperationCanceled => {
                process::exit(0);
            }
            _ => Err(err),
        },
    }
}

fn choose_workspace_package(metadata: &Metadata, keyword: Option<String>) -> Result<&Package> {
    let workspace_packages = metadata.workspace_packages();
    let select_package_options: Vec<&str> =
        workspace_packages.iter().map(|x| x.name.as_str()).collect();
    let ans = if select_package_options.len() == 1 {
        select_package_options[0]
    } else {
        if let Some(keyword) = keyword {
            let matches = fuzzy_match(&select_package_options, &keyword);
            if matches.is_empty() {
                let result =
                    inquire::Select::new("Select workspace package:", select_package_options)
                        .with_starting_filter_input(&keyword)
                        .prompt();
                handle_prompt_result(result)?
            } else {
                matches[0].0
            }
        } else {
            let result =
                inquire::Select::new("Select workspace package:", select_package_options).prompt();
            handle_prompt_result(result)?
        }
    };
    let index = workspace_packages
        .iter()
        .position(|x| x.name.as_str().eq(ans))
        .unwrap();
    let workspace_package = &workspace_packages[index];

    Ok(workspace_package)
}

fn choose_dependency(package: &Package, keyword: Option<String>) -> Result<&Dependency> {
    let dependencies = &package.dependencies;
    let select_dependencies_options: Vec<&str> =
        dependencies.iter().map(|x| x.name.as_str()).collect();
    let ans = match keyword {
        Some(keyword) => {
            let matches = fuzzy_match(&select_dependencies_options, &keyword);
            if matches.is_empty() {
                let result =
                    inquire::Select::new("Select dependency:", select_dependencies_options)
                        .with_starting_filter_input(&keyword)
                        .prompt();
                handle_prompt_result(result)?
            } else {
                matches[0].0
            }
        }
        None => {
            let result =
                inquire::Select::new("Select dependency:", select_dependencies_options).prompt();
            handle_prompt_result(result)?
        }
    };
    let index = dependencies
        .iter()
        .position(|x| x.name.as_str().eq(ans))
        .unwrap();
    let dependency = &dependencies[index];
    Ok(dependency)
}

fn choose_features(metadata: &Metadata, dependency: &Dependency) -> Result<(bool, Vec<String>)> {
    let mut enabled_features = dependency.features.clone();
    let uses_default_features = dependency.uses_default_features;

    let package = metadata
        .packages
        .iter()
        .find(|x| x.name == dependency.name)
        .unwrap();
    let package_features = &package.features;
    let package_default_features = package_features
        .get("default")
        .map(|x| x.iter().cloned().collect::<Vec<String>>());

    if package_default_features.is_some() && uses_default_features {
        enabled_features.insert(0, "default".to_string());
    }

    let mut toggle_features_options = package_features
        .iter()
        .map(|x| Feature {
            name: x.0.to_string(),
            includes: x.1.clone(),
        })
        .collect::<Vec<_>>();

    toggle_features_options.sort_by(|a, b| {
        if a.name.eq("default") {
            Ordering::Less
        } else {
            a.name.cmp(&b.name)
        }
    });

    let default_index = toggle_features_options
        .iter()
        .enumerate()
        .filter(|(_i, x)| enabled_features.contains(&x.name))
        .map(|(i, _x)| i)
        .collect::<Vec<usize>>();

    let result = inquire::MultiSelect::new("Toggle features", toggle_features_options)
        .with_default(&default_index)
        .prompt();
    let ans = handle_prompt_result(result)?;

    let enabled_features = ans.into_iter().map(|x| x.name).collect::<Vec<String>>();
    let uses_default_features =
        package_default_features.is_none() || enabled_features.iter().any(|x| x == "default");

    let enabled_features_except_default = enabled_features
        .into_iter()
        .filter(|x| x.ne(&"default"))
        .collect::<Vec<_>>();

    Ok((uses_default_features, enabled_features_except_default))
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let cli = Cli::parse();

    if let Some(Commands::Features(args)) = cli.command {
        let metadata = MetadataCommand::new().exec().unwrap();

        let workspace_package = choose_workspace_package(&metadata, args.package)?;

        let dependency = choose_dependency(workspace_package, args.dependency)?;
        let dependency_name = dependency.name.as_str();

        let (uses_default_features, enabled_features) = choose_features(&metadata, dependency)?;

        let toml_path = &workspace_package.manifest_path;
        let toml_plaintext = fs::read_to_string(toml_path)?;
        let mut doc = toml_plaintext.parse::<DocumentMut>()?;

        let mut array = toml_edit::Array::default();
        enabled_features.iter().for_each(|f| {
            array.push(f);
        });

        let version = dependency
            .req
            .to_string()
            .trim_start_matches('^')
            .to_owned();

        if uses_default_features && array.is_empty() {
            // only declar version
            doc["dependencies"][dependency_name] = toml_edit::value(version);
        } else {
            // use inline table
            doc["dependencies"][dependency_name] = toml_edit::value(toml_edit::InlineTable::new());
            // set version
            doc["dependencies"][dependency_name]["version"] = toml_edit::value(version);
            if !uses_default_features {
                // set default-features
                doc["dependencies"][dependency_name]["default-features"] = toml_edit::value(false);
            }
            if !array.is_empty() {
                // set features
                doc["dependencies"][dependency_name]["features"] = toml_edit::value(array);
            }
        }

        fs::write(toml_path, doc.to_string())?;
    }

    Ok(())
}
