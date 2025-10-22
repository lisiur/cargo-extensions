use std::{cmp::Ordering, fmt::Display, fs, process};

use anyhow::Result;
use cargo_metadata::{Dependency, Metadata, MetadataCommand, Package};
use clap::{Args, Parser, Subcommand};
use colored::Colorize;
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
    /// Manage workspace dependencies
    Features(FeatureArgs),
}

#[derive(Args)]
struct FeatureArgs {
    /// Workspace package name
    #[arg(short, long, value_name = "PACKAGE")]
    package: Option<String>,

    /// Dependency name
    #[arg(short, long, value_name = "DEPENDENCY")]
    dependency: Option<String>,

    #[command(subcommand)]
    command: Option<FeatureCommands>,
}

#[derive(Subcommand)]
enum FeatureCommands {
    /// List workspace dependencies
    List(FeatureListArgs),
}

#[derive(Args)]
struct FeatureListArgs {
    /// Workspace package name
    #[arg(short, long, value_name = "PACKAGE")]
    package: Option<String>,

    /// Dependency name
    #[arg(short, long, value_name = "DEPENDENCY")]
    dependency: Option<String>,

    #[arg(short, long, default_value_t = false)]
    all: bool,
}

struct Feature {
    name: String,
    includes: Vec<String>,
}

struct DependencyFeatures {
    features: Vec<Feature>,
    enabled_features: Vec<String>,
}

impl DependencyFeatures {
    fn uses_default_features(&self) -> bool {
        self.enabled_features.contains(&"default".to_string())
    }

    fn enabled_features_except_default(&self) -> Vec<&String> {
        self.enabled_features
            .iter()
            .filter(|x| x.ne(&"default"))
            .collect::<Vec<_>>()
    }
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

fn get_dependency_features(metadata: &Metadata, dependency: &Dependency) -> DependencyFeatures {
    let mut enabled_features = dependency.features.clone();

    let dep_package = metadata
        .packages
        .iter()
        .find(|x| x.name == dependency.name)
        .unwrap();

    if dep_package.features.get("default").is_some() && dependency.uses_default_features {
        enabled_features.insert(0, "default".to_string());
    }

    let mut all_features = dep_package
        .features
        .iter()
        .map(|x| Feature {
            name: x.0.to_string(),
            includes: x.1.clone(),
        })
        .collect::<Vec<_>>();

    all_features.sort_by(|a, b| {
        if a.name.eq("default") {
            Ordering::Less
        } else {
            a.name.cmp(&b.name)
        }
    });

    DependencyFeatures {
        features: all_features,
        enabled_features,
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

fn choose_features(metadata: &Metadata, dependency: &Dependency) -> Result<DependencyFeatures> {
    let mut dependency_features = get_dependency_features(metadata, dependency);

    let toggle_features_options = dependency_features.features.iter().collect::<Vec<_>>();

    let default_index = toggle_features_options
        .iter()
        .enumerate()
        .filter(|(_i, x)| dependency_features.enabled_features.contains(&x.name))
        .map(|(i, _x)| i)
        .collect::<Vec<usize>>();

    let result = inquire::MultiSelect::new("Toggle features", toggle_features_options)
        .with_default(&default_index)
        .prompt();

    let ans = handle_prompt_result(result)?;

    let enabled_features = ans
        .into_iter()
        .map(|x| x.name.clone())
        .collect::<Vec<String>>();

    dependency_features.enabled_features = enabled_features;

    Ok(dependency_features)
}

fn manage_features(package: Option<String>, dependency: Option<String>) -> Result<()> {
    let metadata = MetadataCommand::new().exec()?;

    let workspace_package = choose_workspace_package(&metadata, package)?;

    let dependency = choose_dependency(workspace_package, dependency)?;
    let dependency_name = dependency.name.as_str();

    let dependency_features = choose_features(&metadata, dependency)?;
    let uses_default_features = dependency_features.uses_default_features();

    let toml_path = &workspace_package.manifest_path;
    let toml_plaintext = fs::read_to_string(toml_path)?;
    let mut doc = toml_plaintext.parse::<DocumentMut>()?;

    let mut array = toml_edit::Array::default();
    dependency_features
        .enabled_features_except_default()
        .into_iter()
        .for_each(|f| {
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

    Ok(())
}

fn list_workspace_features(args: FeatureListArgs) -> Result<()> {
    let metadata = MetadataCommand::new().exec()?;
    let workspace_packages = metadata.workspace_packages();
    for package in workspace_packages {
        if args
            .package
            .as_ref()
            .map_or(false, |x| !x.contains(&package.name.to_string()))
        {
            continue;
        }
        println!("{:>0}{}:", "", package.name.cyan());
        let dependencies = &package.dependencies;
        for dependency in dependencies {
            if args
                .dependency
                .as_ref()
                .map_or(false, |x| !x.contains(&dependency.name))
            {
                continue;
            }

            println!("{:>2}{}:", "", dependency.name);
            let dependency_features = get_dependency_features(&metadata, dependency);
            if args.all {
                for feature in dependency_features.features {
                    let enabled = dependency_features.enabled_features.contains(&feature.name);
                    println!(
                        "{:>4}{} {}",
                        "",
                        format!(
                            "{}{}{}",
                            "[".bright_black(),
                            if enabled {
                                "x".green().to_string()
                            } else {
                                " ".to_string()
                            },
                            "]".bright_black()
                        ),
                        feature
                    );
                }
            } else {
                for feature in dependency_features.enabled_features {
                    println!(
                        "{:>4}{}",
                        "",
                        Feature {
                            name: feature,
                            includes: dependency.features.clone(),
                        }
                    );
                }
            }
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Commands::Features(args)) = cli.command {
        if let Some(feat_command) = args.command {
            match feat_command {
                FeatureCommands::List(list_args) => {
                    list_workspace_features(list_args)?;
                }
            }
            return Ok(());
        } else {
            manage_features(args.package, args.dependency)?;
        }
    }

    Ok(())
}
