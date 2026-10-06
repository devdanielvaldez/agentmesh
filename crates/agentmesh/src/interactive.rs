//! Full-screen, keyboard-driven AgentMesh console.

use std::{
    collections::BTreeSet,
    io::{self, IsTerminal, Read, Write},
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
};

use agentmesh_protocol::{CapabilityPackage, discover_capabilities};
use anyhow::{Context, Result};

use crate::{CapabilityAction, capability_cli};

const WIDTH: usize = 88;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Capabilities,
    CapabilityDetail,
    Workflows,
    WorkflowDetail,
    Composites,
    CompositeDetail,
    Integrations,
    Sessions,
    Diagnostics,
    Help,
}

#[derive(Debug)]
struct App {
    page: Page,
    home_selection: usize,
    selected: usize,
    filter: String,
    selected_capability: Option<CapabilityPackage>,
    selected_workflow: Option<String>,
    selected_composite: Option<String>,
    catalog_root: PathBuf,
    notice: String,
}

impl App {
    fn new() -> Self {
        Self {
            page: Page::Home,
            home_selection: 0,
            selected: 0,
            filter: String::new(),
            selected_capability: None,
            selected_workflow: None,
            selected_composite: None,
            catalog_root: capability_cli::catalog_directory(),
            notice: "AgentMesh is ready. Select a workspace to get started.".into(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let length = match self.page {
            Page::Home => 7,
            Page::Capabilities => visible_capabilities(self).len(),
            Page::Workflows => workflow_summaries().map_or(0, |items| items.len()),
            Page::Composites => {
                crate::composite_cli::console_catalog().map_or(0, |items| items.len())
            }
            _ => 0,
        };
        if length == 0 {
            self.selected = 0;
        } else {
            self.selected = (self.selected as isize + delta).rem_euclid(length as isize) as usize;
        }
    }

    fn back(&mut self) {
        self.page = match self.page {
            Page::CapabilityDetail => Page::Capabilities,
            Page::WorkflowDetail => Page::Workflows,
            Page::CompositeDetail => Page::Composites,
            _ => Page::Home,
        };
        self.notice = "Ready".into();
    }
}

pub(crate) fn run() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        anyhow::bail!("The AgentMesh console requires an interactive terminal.");
    }
    let mut app = App::new();
    loop {
        let key = {
            let _terminal = TerminalGuard::enter()?;
            draw(&app);
            read_key()?
        };
        if !handle_key(&mut app, key)? {
            break;
        }
    }
    println!("AgentMesh console closed.");
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
    Up,
    Down,
    Enter,
    Escape,
    Character(char),
    Other,
}

fn read_key() -> Result<Key> {
    #[cfg(unix)]
    {
        let mut stdin = io::stdin().lock();
        let mut first = [0_u8; 1];
        loop {
            if stdin.read(&mut first)? == 1 {
                break;
            }
        }
        return Ok(match first[0] {
            b'\r' | b'\n' => Key::Enter,
            3 | 4 => Key::Escape,
            27 => {
                let mut sequence = [0_u8; 1];
                if stdin.read(&mut sequence)? == 1 && sequence[0] == b'[' {
                    if stdin.read(&mut sequence)? == 0 {
                        return Ok(Key::Escape);
                    }
                    match sequence[0] {
                        b'A' => Key::Up,
                        b'B' => Key::Down,
                        _ => Key::Other,
                    }
                } else {
                    Key::Escape
                }
            }
            byte if byte.is_ascii() => Key::Character(char::from(byte)),
            _ => Key::Other,
        });
    }
    #[cfg(not(unix))]
    {
        print!("\nSelect action (number, Enter, or q): ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        Ok(match input.trim() {
            "1" => Key::Character('1'),
            "2" => Key::Character('2'),
            "3" => Key::Character('3'),
            "4" => Key::Character('4'),
            "5" => Key::Character('5'),
            "6" => Key::Character('6'),
            "7" => Key::Character('7'),
            "8" => Key::Character('8'),
            "q" | "Q" => Key::Character('q'),
            _ => Key::Enter,
        })
    }
}

fn handle_key(app: &mut App, key: Key) -> Result<bool> {
    if key == Key::Escape {
        if app.page == Page::Home {
            return Ok(false);
        }
        app.back();
        return Ok(true);
    }
    match app.page {
        Page::Home => match key {
            Key::Up => app.home_selection = (app.home_selection + 7) % 8,
            Key::Down => app.home_selection = (app.home_selection + 1) % 8,
            Key::Character('1'..='8') => {
                app.home_selection = key_digit(key) - 1;
                open_home_page(app);
            }
            Key::Enter => open_home_page(app),
            Key::Character('q' | 'Q') => return Ok(false),
            _ => {}
        },
        Page::Capabilities => match key {
            Key::Up => app.move_selection(-1),
            Key::Down => app.move_selection(1),
            Key::Enter => {
                let matches = visible_capabilities(app);
                if let Some(package) = matches.get(app.selected) {
                    app.selected_capability = Some((*package).clone());
                    app.page = Page::CapabilityDetail;
                }
            }
            Key::Character('/') => {
                app.filter = prompt("Search capability intent")?;
                app.selected = 0;
            }
            Key::Character('c' | 'C') => {
                app.filter.clear();
                app.selected = 0;
            }
            Key::Character('i' | 'I') => install_package_from_path(app)?,
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::CapabilityDetail => match key {
            Key::Character('v' | 'V') => {
                if let Some(package) = &app.selected_capability {
                    package.validate().context("Capability validation failed")?;
                    app.notice = format!(
                        "Validated {}@{}",
                        package.capability.id, package.capability.version
                    );
                }
                pause_for_action();
            }
            Key::Character('i' | 'I') => install_package_from_path(app)?,
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::Workflows => match key {
            Key::Up => app.move_selection(-1),
            Key::Down => app.move_selection(1),
            Key::Enter => {
                if let Some(summary) = workflow_summaries()?.get(app.selected) {
                    app.selected_workflow = Some(summary.id.clone());
                    app.page = Page::WorkflowDetail;
                }
            }
            Key::Character('t' | 'T') => teach_browser_from_console()?,
            Key::Character('r' | 'R') => run_selected_workflow(app)?,
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::WorkflowDetail => match key {
            Key::Character('r' | 'R') => run_selected_workflow(app)?,
            Key::Character('m' | 'M') => {
                if let Some(id) = &app.selected_workflow {
                    publish_workflow_from_console(id)?;
                }
            }
            Key::Character('v' | 'V') => {
                if let Some(id) = &app.selected_workflow {
                    run_action(&format!("Validate workflow {id}"), || {
                        crate::workflows_validate(id)
                    })?;
                }
            }
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::Composites => match key {
            Key::Up => app.move_selection(-1),
            Key::Down => app.move_selection(1),
            Key::Enter => {
                if let Some(item) = crate::composite_cli::console_catalog()?.get(app.selected) {
                    app.selected_composite = Some(item.id.clone());
                    app.page = Page::CompositeDetail;
                }
            }
            Key::Character('c' | 'C') => create_composite_from_console()?,
            Key::Character('r' | 'R') => run_selected_composite(app)?,
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::CompositeDetail => match key {
            Key::Character('r' | 'R') => run_selected_composite(app)?,
            Key::Character('c' | 'C') => create_composite_from_console()?,
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::Integrations => match key {
            Key::Character('r' | 'R') => {
                app.notice = "Configuration reloaded on the next view.".into()
            }
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::Sessions => match key {
            Key::Character('l' | 'L') => login_session_from_console()?,
            Key::Character('r' | 'R') => app.notice = "Session list refreshed.".into(),
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::Diagnostics => match key {
            Key::Enter | Key::Character('r' | 'R') => {
                let path = PathBuf::from("config/agentmesh.yaml");
                run_action("AgentMesh diagnostics", || crate::doctor(&path))?;
            }
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
        Page::Help => match key {
            Key::Character('q' | 'Q') => app.back(),
            _ => {}
        },
    }
    Ok(true)
}

fn key_digit(key: Key) -> usize {
    match key {
        Key::Character(character) => character.to_digit(10).unwrap_or(1) as usize,
        _ => 1,
    }
}

fn open_home_page(app: &mut App) {
    app.selected = 0;
    app.page = match app.home_selection {
        0 => Page::Home,
        1 => Page::Capabilities,
        2 => Page::Workflows,
        3 => Page::Composites,
        4 => Page::Integrations,
        5 => Page::Sessions,
        6 => Page::Diagnostics,
        _ => Page::Help,
    };
}

fn draw(app: &App) {
    let mut output = String::from("\x1b[2J\x1b[H\x1b[1;36m");
    output.push_str("  ▄▀█ █▀▀ █▀▀ █▄ █ ▀█▀ █▀▄ █▀▀ █▀ █  ██\n");
    output.push_str("  █▀█ █▄▄ ██▄ █ ▀█  █  █▄▀ ██▄ ▄█ █▄▄ ▄▄\x1b[0m\n");
    output.push_str(&format!(
        "  CAPABILITY WORKSPACE  ·  v{}\n",
        env!("CARGO_PKG_VERSION")
    ));
    output.push_str(&format!("{}\n", "─".repeat(WIDTH)));

    if app.page == Page::Home {
        draw_home(app, &mut output);
    } else {
        draw_sidebar(app, &mut output);
        output.push_str(&format!("{}\n", "─".repeat(WIDTH)));
        match app.page {
            Page::Capabilities => draw_capabilities(app, &mut output),
            Page::CapabilityDetail => draw_capability_detail(app, &mut output),
            Page::Workflows => draw_workflows(app, &mut output),
            Page::WorkflowDetail => draw_workflow_detail(app, &mut output),
            Page::Composites => draw_composites(app, &mut output),
            Page::CompositeDetail => draw_composite_detail(app, &mut output),
            Page::Integrations => draw_integrations(&mut output),
            Page::Sessions => draw_sessions(&mut output),
            Page::Diagnostics => draw_diagnostics(&mut output),
            Page::Help => draw_help(&mut output),
            Page::Home => {}
        }
    }
    output.push_str(&format!("{}\n", "─".repeat(WIDTH)));
    output.push_str(&format!("  {}\n", app.notice));
    output.push_str("  ↑↓ Navigate   Enter Open   Esc Back   q Quit   / Search\n");
    print!("{output}");
    let _ = io::stdout().flush();
}

fn draw_home(app: &App, output: &mut String) {
    output.push_str("  Your agent operations, all in one place.\n\n");
    let items = [
        ("Overview", "Live inventory and activity at a glance"),
        (
            "Capabilities",
            "Browse packages and visualize dependency graphs",
        ),
        (
            "Workflows & Teach",
            "Learn, inspect, validate, and run workflows",
        ),
        (
            "Composite MCP tools",
            "Build and run multi-step agent tools",
        ),
        (
            "MCP Integrations",
            "Inspect configured MCP providers and routing",
        ),
        (
            "Login Sessions",
            "Create and reuse authenticated browser profiles",
        ),
        (
            "Diagnostics",
            "Check configuration and runtime prerequisites",
        ),
        ("Help & Shortcuts", "Keyboard guide and command reference"),
    ];
    for (index, (title, subtitle)) in items.iter().enumerate() {
        let marker = if index == app.home_selection {
            "▶"
        } else {
            " "
        };
        output.push_str(&format!(
            "  {marker}  {}  {title:<23} {subtitle}\n",
            index + 1
        ));
    }
    output.push('\n');
    let workflows = workflow_summaries().map_or(0, |items| items.len());
    let composites = crate::composite_cli::console_catalog().map_or(0, |items| items.len());
    let capabilities =
        capability_cli::load_catalog(&app.catalog_root).map_or(0, |items| items.len());
    let providers = configured_upstreams().map_or(0, |items| items.len());
    output.push_str(
        "  ┌─ WORKSPACE SNAPSHOT ──────────────────────────────────────────────────────┐\n",
    );
    output.push_str(&format!(
        "  │  Workflows  {:>3}     Composites  {:>3}     Capabilities  {:>3}     MCP  {:>3}    │\n",
        workflows, composites, capabilities, providers
    ));
    output.push_str(
        "  └──────────────────────────────────────────────────────────────────────────┘\n\n",
    );
    draw_inventory_bars(workflows, capabilities, providers, output);
}

fn draw_sidebar(app: &App, output: &mut String) {
    let pages = [
        "Home",
        "Capabilities",
        "Workflows",
        "Composites",
        "Integrations",
        "Sessions",
        "Diagnostics",
        "Help",
    ];
    let active = match app.page {
        Page::Home => 0,
        Page::Capabilities | Page::CapabilityDetail => 1,
        Page::Workflows | Page::WorkflowDetail => 2,
        Page::Composites | Page::CompositeDetail => 3,
        Page::Integrations => 4,
        Page::Sessions => 5,
        Page::Diagnostics => 6,
        Page::Help => 7,
    };
    output.push_str("  ");
    for (index, page) in pages.iter().enumerate() {
        let token = if index == active {
            format!("\x1b[1;36m {page} \x1b[0m")
        } else {
            format!(" {page} ")
        };
        output.push_str(&token);
        output.push('│');
    }
    output.push('\n');
}

fn draw_capabilities(app: &App, output: &mut String) {
    let packages = match capability_cli::load_catalog(&app.catalog_root) {
        Ok(packages) => packages,
        Err(error) => {
            output.push_str(&format!("  Could not read capability catalog: {error}\n"));
            return;
        }
    };
    let matches = discover_capabilities(&app.filter, &packages, 10_000);
    output.push_str("  CAPABILITY CATALOG\n");
    output.push_str(&format!(
        "  {} package(s)   Catalog: {}\n",
        packages.len(),
        app.catalog_root.display()
    ));
    output.push_str(&format!(
        "  Search: {}  ( / to edit · c to clear )\n\n",
        if app.filter.is_empty() {
            "all"
        } else {
            &app.filter
        }
    ));
    if matches.is_empty() {
        output.push_str("  No packages match. Press i to install a package file.\n");
        return;
    }
    for (index, result) in matches.iter().enumerate().take(18) {
        let marker = if index == app.selected { "▶" } else { " " };
        output.push_str(&format!(
            "  {marker}  {:<30} {:<12} {}\n",
            result.package.capability.id,
            result.package.capability.version,
            result.package.capability.intent
        ));
    }
    output.push_str("\n  Enter graph details  ·  i install package  ·  v validate in details\n");
}

fn visible_capabilities(app: &App) -> Vec<CapabilityPackage> {
    let Ok(packages) = capability_cli::load_catalog(&app.catalog_root) else {
        return Vec::new();
    };
    discover_capabilities(&app.filter, &packages, 10_000)
        .into_iter()
        .map(|result| result.package.clone())
        .collect()
}

fn draw_capability_detail(app: &App, output: &mut String) {
    let Some(package) = &app.selected_capability else {
        output.push_str("  No capability selected.\n");
        return;
    };
    output.push_str(&format!(
        "  {}  @  {}\n  {}\n\n",
        package.capability.id, package.capability.version, package.capability.intent
    ));
    output.push_str("  CAPABILITY MAP\n  ┌─ ");
    output.push_str(&format!("{}\n", package.capability.id));
    let effects = package
        .contract
        .effects
        .iter()
        .map(|effect| format!("{effect:?}").to_lowercase())
        .collect::<Vec<_>>()
        .join(", ");
    output.push_str(&format!("  ├── effects       {effects}\n"));
    output.push_str(&format!(
        "  ├── permissions   {}\n",
        if package.authority.permissions.is_empty() {
            "none".into()
        } else {
            package.authority.permissions.join(", ")
        }
    ));
    output.push_str(&format!(
        "  ├── verification  {} evidence claim(s)\n",
        package.contract.success_evidence.len()
    ));
    output.push_str(&format!(
        "  ├── recovery      {:?}\n",
        package.contract.recovery
    ));
    output.push_str("  ├── bindings\n");
    for (index, binding) in package.implementations.iter().enumerate() {
        let final_item =
            index + 1 == package.implementations.len() && package.contract.requires.is_empty();
        output.push_str(&format!(
            "  │   {}── {}  ({:?})\n",
            if final_item { "└" } else { "├" },
            binding.id,
            binding.kind
        ));
    }
    if !package.contract.requires.is_empty() {
        output.push_str("  └── requires\n");
        draw_dependency_graph(
            package,
            &app.catalog_root,
            "      ",
            &mut BTreeSet::new(),
            output,
        );
    }
    output.push_str("\n  v validate package   i install another   q back\n");
}

fn draw_dependency_graph(
    package: &CapabilityPackage,
    root: &PathBuf,
    prefix: &str,
    visited: &mut BTreeSet<String>,
    output: &mut String,
) {
    for (index, requirement) in package.contract.requires.iter().enumerate() {
        let is_last = index + 1 == package.contract.requires.len();
        let version = requirement.version.as_deref().unwrap_or("any");
        output.push_str(&format!(
            "{prefix}{} {}@{version}\n",
            if is_last { "└──" } else { "├──" },
            requirement.id
        ));
        if !visited.insert(requirement.id.clone()) {
            continue;
        }
        let Ok(catalog) = capability_cli::load_catalog(root) else {
            continue;
        };
        if let Some(dependency) = catalog.iter().find(|candidate| {
            candidate.capability.id == requirement.id
                && requirement
                    .version
                    .as_deref()
                    .is_none_or(|version| candidate.capability.version == version)
        }) {
            let child_prefix = format!("{prefix}{}   ", if is_last { "    " } else { "│   " });
            draw_dependency_graph(dependency, root, &child_prefix, visited, output);
        }
    }
}

fn draw_workflows(app: &App, output: &mut String) {
    let summaries = match workflow_summaries() {
        Ok(items) => items,
        Err(error) => {
            output.push_str(&format!("  Could not read workflows: {error}\n"));
            return;
        }
    };
    output.push_str(&format!(
        "  WORKFLOW LIBRARY  ·  {} workflow(s)\n\n",
        summaries.len()
    ));
    if summaries.is_empty() {
        output
            .push_str("  Your library is empty. Press t to record your first browser workflow.\n");
    }
    for (index, workflow) in summaries.iter().enumerate().take(18) {
        let marker = if index == app.selected { "▶" } else { " " };
        output.push_str(&format!(
            "  {marker}  {:<34} {:<10} {:>2} steps  {}\n",
            workflow.id, workflow.runtime, workflow.steps, workflow.description
        ));
    }
    output.push_str("\n  Enter inspect  ·  r run selected  ·  t record a browser session\n");
}

fn draw_composites(app: &App, output: &mut String) {
    let items = match crate::composite_cli::console_catalog() {
        Ok(items) => items,
        Err(error) => {
            output.push_str(&format!("  Could not read composite catalog: {error}\n"));
            return;
        }
    };
    output.push_str(&format!(
        "  COMPOSITE MCP TOOLS  ·  {} saved\n\n",
        items.len()
    ));
    if items.is_empty() {
        output.push_str("  No composites yet. Press c to create one from a JSON/YAML recipe.\n");
    }
    for (index, item) in items.iter().enumerate().take(18) {
        let marker = if index == app.selected { "▶" } else { " " };
        output.push_str(&format!(
            "  {marker}  {:<34} {:>2} steps  {:<8} {}\n",
            item.id, item.step_count, item.risk, item.description
        ));
    }
    output.push_str("\n  Enter inspect  ·  r run selected  ·  c create from recipe\n");
}

fn draw_composite_detail(app: &App, output: &mut String) {
    let Some(id) = &app.selected_composite else {
        output.push_str("  No composite selected.\n");
        return;
    };
    match crate::composite_cli::console_catalog().and_then(|items| {
        items
            .into_iter()
            .find(|item| &item.id == id)
            .context("composite not found")
    }) {
        Ok(item) => {
            output.push_str(&format!("  {}\n  {}\n\n", item.id, item.description));
            output.push_str(&format!(
                "  Steps: {}    Risk: {}    Writes: {}\n\n  INPUT SCHEMA\n",
                item.step_count,
                item.risk,
                if item.has_write { "yes" } else { "no" }
            ));
            output.push_str(&format!(
                "{}\n",
                serde_json::to_string_pretty(&item.inputs).unwrap_or_default()
            ));
            output.push_str("\n  r run composite   c create another   q back\n");
        }
        Err(error) => output.push_str(&format!("  Could not inspect composite: {error}\n")),
    }
}

fn create_composite_from_console() -> Result<()> {
    let recipe = prompt("JSON/YAML recipe path")?;
    if recipe.trim().is_empty() {
        return Ok(());
    }
    run_action("Create composite MCP tool", || {
        crate::composite_cli::run(
            crate::CompositeAction::Create {
                recipe: PathBuf::from(recipe),
            },
            None,
        )
    })
}

fn run_selected_composite(app: &App) -> Result<()> {
    let items = crate::composite_cli::console_catalog()?;
    let id = app
        .selected_composite
        .clone()
        .or_else(|| items.get(app.selected).map(|item| item.id.clone()));
    let Some(id) = id else {
        app_notice("No composite is selected.");
        return Ok(());
    };
    let item = items
        .iter()
        .find(|item| item.id == id)
        .context("composite not found")?;
    let mut input = Vec::new();
    if let Some(properties) = item
        .inputs
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        let required = item
            .inputs
            .get("required")
            .and_then(serde_json::Value::as_array);
        for (name, schema) in properties {
            let default = schema
                .get("default")
                .map(|value| {
                    value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_owned)
                })
                .unwrap_or_default();
            let is_required = required
                .is_some_and(|items| items.iter().any(|value| value.as_str() == Some(name)));
            let label = if !default.is_empty() {
                format!("{name} [default: {default}]")
            } else if is_required {
                format!("{name} (required)")
            } else {
                format!("{name} (optional; empty to skip)")
            };
            let value = prompt(&label)?;
            let value = if value.is_empty() { default } else { value };
            if value.is_empty() && is_required {
                anyhow::bail!("required composite input {name:?} was empty");
            }
            if !value.is_empty() {
                input.push(format!("{name}={value}"));
            }
        }
    }
    let profiles = session_profiles();
    let session = if profiles.is_empty() {
        None
    } else {
        let choice = prompt("Saved login profile (empty for none)")?;
        profiles
            .iter()
            .find(|profile| profile.0 == choice)
            .map(|profile| profile.0.clone())
    };
    let headed = matches!(
        prompt("Show browser windows? [y/N]")?
            .trim()
            .to_lowercase()
            .as_str(),
        "y" | "yes"
    );
    run_action(&format!("Run composite {id}"), || {
        crate::composite_cli::run(
            crate::CompositeAction::Run {
                id,
                input,
                yes: false,
                session,
                headed,
            },
            None,
        )
    })
}

fn draw_workflow_detail(app: &App, output: &mut String) {
    let Some(id) = &app.selected_workflow else {
        output.push_str("  No workflow selected.\n");
        return;
    };
    match agentmesh_teach::load_workflow(id) {
        Ok(workflow) => {
            output.push_str(&format!(
                "  {}\n  {}\n\n",
                workflow.id, workflow.description
            ));
            output.push_str(&format!(
                "  Runtime: {}    Steps: {}    Inputs: {}    Outputs: {}\n",
                workflow.runtime,
                workflow.steps.len(),
                workflow.inputs.len(),
                workflow.outputs.len()
            ));
            output.push_str("\n  EXECUTION FLOW\n");
            for (index, step) in workflow.steps.iter().enumerate().take(12) {
                output.push_str(&format!(
                    "  {:>2}.  {:<24} {}\n",
                    index + 1,
                    step.id,
                    step.op
                ));
            }
            output.push_str(
                "\n  r run workflow   m create MCP / add to a client   v validate   q back\n",
            );
        }
        Err(error) => output.push_str(&format!("  Could not load workflow: {error}\n")),
    }
}

fn draw_integrations(output: &mut String) {
    output.push_str("  MCP INTEGRATIONS\n\n");
    match configured_upstreams() {
        Ok(upstreams) if upstreams.is_empty() => {
            output.push_str("  No upstream providers are configured.\n")
        }
        Ok(upstreams) => {
            for (index, upstream) in upstreams.iter().enumerate() {
                output.push_str(&format!(
                    "  {:>2}.  {:<24} {}\n",
                    index + 1,
                    upstream.effective_name(index),
                    upstream.url
                ));
            }
        }
        Err(error) => output.push_str(&format!("  Configuration could not be loaded: {error}\n")),
    }
    output.push_str("\n  Gateway configuration: config/agentmesh.yaml\n  Edit that file, then restart the gateway to apply changes.\n");
}

fn draw_sessions(output: &mut String) {
    output.push_str("  BROWSER LOGIN SESSIONS\n\n");
    let profiles = session_profiles();
    if profiles.is_empty() {
        output.push_str("  No saved login profiles yet.\n");
    } else {
        for (index, (name, active, path)) in profiles.iter().enumerate() {
            output.push_str(&format!(
                "  {:>2}.  {:<24} {:<10} {}\n",
                index + 1,
                name,
                if *active { "logged in" } else { "empty" },
                path.display()
            ));
        }
    }
    output.push_str("\n  l create / sign in to a profile   r refresh   q back\n");
    output.push_str("  Choose a saved profile while recording with t in Workflows & Teach.\n");
}

fn session_profiles() -> Vec<(String, bool, PathBuf)> {
    let Some(home) = agentmesh_teach::home_dir() else {
        return Vec::new();
    };
    let sessions = home.join("sessions");
    let Ok(entries) = std::fs::read_dir(sessions) else {
        return Vec::new();
    };
    let mut profiles: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let active = path.join("Default").join("Preferences").is_file();
            (name, active, path)
        })
        .collect();
    profiles.sort_by(|left, right| left.0.cmp(&right.0));
    profiles
}

fn login_session_from_console() -> Result<()> {
    let name = prompt("Application/profile name (e.g. work_portal)")?;
    if name.trim().is_empty() {
        return Ok(());
    }
    let url = prompt("Login page URL")?;
    if url.trim().is_empty() {
        println!("Login URL is required.");
        pause_for_action();
        return Ok(());
    }
    run_action(&format!("Open login session {name}"), || {
        crate::teach_flow::sessions_login(&name, &url)
    })
}

fn teach_browser_from_console() -> Result<()> {
    let name = prompt("Workflow id (e.g. shop.create_receipt; empty to enter later)")?;
    let start_url = prompt("Starting page URL (optional)")?;
    let scope = prompt("Allowed site origin (optional, e.g. https://example.com)")?;
    println!("\nLogin for this recording:");
    println!("  1. Temporary browser, no saved login");
    println!("  2. Reuse a saved login profile");
    println!("  3. Sign in now and save a profile");
    let choice = prompt("Choose 1, 2, or 3 [1]")?;
    let session = match choice.trim() {
        "" | "1" => None,
        "2" => {
            let profiles = session_profiles();
            if profiles.is_empty() {
                println!("No saved profiles found; choose 3 to create one.");
                pause_for_action();
                return Ok(());
            }
            println!("Saved profiles:");
            for (index, (profile, active, _)) in profiles.iter().enumerate() {
                println!(
                    "  {}. {}{}",
                    index + 1,
                    profile,
                    if *active {
                        " (login data found)"
                    } else {
                        " (login may be missing)"
                    }
                );
            }
            let selection = prompt("Profile number or name")?;
            let profile = selection
                .parse::<usize>()
                .ok()
                .and_then(|number| number.checked_sub(1))
                .and_then(|index| profiles.get(index).map(|item| item.0.clone()))
                .or_else(|| {
                    profiles
                        .iter()
                        .find(|item| item.0 == selection)
                        .map(|item| item.0.clone())
                });
            let Some(profile) = profile else {
                println!("That profile was not found.");
                pause_for_action();
                return Ok(());
            };
            Some(profile)
        }
        "3" => {
            let profile = prompt("Name for the saved login profile")?;
            if profile.trim().is_empty() {
                println!("Profile name is required.");
                pause_for_action();
                return Ok(());
            }
            let login_url = if !start_url.trim().is_empty() {
                start_url.clone()
            } else {
                prompt("Login page URL")?
            };
            if login_url.trim().is_empty() {
                println!("Login URL is required.");
                pause_for_action();
                return Ok(());
            }
            run_action(&format!("Sign in and save profile {profile}"), || {
                crate::teach_flow::sessions_login(&profile, &login_url)
            })?;
            Some(profile)
        }
        other => {
            println!("Unknown choice {other:?}; recording cancelled.");
            pause_for_action();
            return Ok(());
        }
    };
    run_action("Record browser workflow", || {
        crate::dispatch_teach(crate::Command::Teach {
            name: (!name.trim().is_empty()).then_some(name),
            target: "browser".into(),
            scope: (!scope.trim().is_empty()).then_some(scope),
            start_url: (!start_url.trim().is_empty()).then_some(start_url),
            headed: true,
            headless: false,
            session,
            continue_from: None,
            cdp_port: None,
            raw: false,
        })
    })
}

fn publish_workflow_from_console(id: &str) -> Result<()> {
    let export_all = matches!(
        prompt("Export this workflow only? [Y/n]")?
            .trim()
            .to_lowercase()
            .as_str(),
        "n" | "no"
    );
    println!("\nWhere should AgentMesh connect the MCP server?");
    println!("  1. Claude Code — this project");
    println!("  2. Claude Code — all projects (requires claude CLI)");
    println!("  3. Claude Desktop — merge its local MCP config");
    println!("  4. Another MCP client — write a config file");
    let client = prompt("Choose 1–4 [1]")?;
    let client = if client.trim().is_empty() {
        "1"
    } else {
        client.trim()
    };
    if !matches!(client, "1" | "2" | "3" | "4") {
        println!("Unknown choice {client:?}; publishing cancelled.");
        pause_for_action();
        return Ok(());
    }
    let profile = choose_login_profile()?;
    let install_dependencies = !matches!(
        prompt("Install MCP server dependencies now? [Y/n]")?
            .trim()
            .to_lowercase()
            .as_str(),
        "n" | "no"
    );

    let result = publish_workflow(
        id,
        export_all,
        client,
        profile.as_deref(),
        install_dependencies,
    );
    match result {
        Ok(()) => println!("\n✓ MCP setup complete."),
        Err(error) => println!("\n✗ MCP setup stopped: {error:#}"),
    }
    pause_for_action();
    Ok(())
}

fn choose_login_profile() -> Result<Option<String>> {
    let profiles = session_profiles();
    if profiles.is_empty() {
        println!("No saved login profiles; continuing without browser login.");
        return Ok(None);
    }
    println!("\nLogin profile for the MCP server (optional):");
    for (index, (name, active, _)) in profiles.iter().enumerate() {
        println!(
            "  {}. {}{}",
            index + 1,
            name,
            if *active {
                " (login data found)"
            } else {
                " (login may be missing)"
            }
        );
    }
    let selection = prompt("Profile number/name, or empty for no login")?;
    if selection.trim().is_empty() {
        return Ok(None);
    }
    let selected = selection
        .parse::<usize>()
        .ok()
        .and_then(|number| number.checked_sub(1))
        .and_then(|index| profiles.get(index))
        .or_else(|| profiles.iter().find(|item| item.0 == selection));
    selected
        .map(|item| Some(item.0.clone()))
        .ok_or_else(|| anyhow::anyhow!("login profile {selection:?} was not found"))
}

fn publish_workflow(
    id: &str,
    export_all: bool,
    client: &str,
    profile: Option<&str>,
    install_dependencies: bool,
) -> Result<()> {
    let home = agentmesh_teach::home_dir().context("no AgentMesh data home")?;
    let server_name = if export_all {
        "agentmesh-taught-capabilities".to_string()
    } else {
        format!("{}-mcp", id.replace('.', "-"))
    };
    let root = home.join("mcp").join(&server_name);
    if root.join("server.mjs").is_file()
        && !matches!(
            prompt(&format!(
                "An MCP export already exists at {}. Refresh generated files? [y/N]",
                root.display()
            ))?
            .trim()
            .to_lowercase()
            .as_str(),
            "y" | "yes"
        )
    {
        anyhow::bail!("kept the existing MCP export unchanged");
    }
    let root_string = root.to_string_lossy().into_owned();
    crate::teach_flow::workflows_export(
        (!export_all).then_some(id),
        export_all,
        "mcp",
        Some(&root_string),
    )?;

    if install_dependencies {
        println!("\nInstalling MCP server dependencies in {}", root.display());
        let status = ProcessCommand::new("npm")
            .arg("install")
            .current_dir(&root)
            .status()
            .context("npm was not found; install Node.js 20+ or skip dependency installation")?;
        if !status.success() {
            anyhow::bail!("npm install exited with {status}");
        }
    }

    let server = root.join("server.mjs");
    let profile_path = profile
        .and_then(|name| agentmesh_teach::home_dir().map(|home| home.join("sessions").join(name)));
    let config = agentmesh_teach::client_config_json(
        &server_name,
        &server
            .canonicalize()
            .context("generated MCP server is missing")?,
        profile_path.as_deref(),
    );
    match client {
        "1" => merge_client_config(&PathBuf::from(".mcp.json"), &config),
        "2" => add_claude_user_server(
            &server_name,
            &server.canonicalize()?,
            profile_path.as_deref(),
        ),
        "3" => merge_client_config(&claude_desktop_config_path()?, &config),
        "4" => {
            let destination = prompt("Path for the generic MCP config file")?;
            if destination.trim().is_empty() {
                anyhow::bail!("configuration output path is required");
            }
            write_client_config(&PathBuf::from(destination), &config)
        }
        _ => unreachable!("choice was validated"),
    }
}

fn add_claude_user_server(
    server_name: &str,
    server: &std::path::Path,
    profile: Option<&std::path::Path>,
) -> Result<()> {
    let mut command = ProcessCommand::new("claude");
    command
        .arg("mcp")
        .arg("add")
        .arg(server_name)
        .arg("--scope")
        .arg("user");
    if let Some(profile) = profile {
        command
            .arg("--env")
            .arg(format!("AGENTMESH_TEACH_PROFILE={}", profile.display()));
    }
    let status =
        command.arg("--").arg("node").arg(server).status().context(
            "Claude Code CLI was not found; choose project scope or install Claude Code",
        )?;
    if !status.success() {
        anyhow::bail!("Claude Code could not add the MCP server (exit {status})");
    }
    println!("Added {server_name} to Claude Code for all projects.");
    Ok(())
}

fn claude_desktop_config_path() -> Result<PathBuf> {
    let home = agentmesh_teach::home_dir().context("no user data directory found")?;
    let path = match std::env::consts::OS {
        "macos" => home
            .ancestors()
            .find(|path| path.ends_with("Application Support/AgentMesh"))
            .map(|path| path.parent().unwrap_or(path).join("Claude"))
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                    .join("Library/Application Support/Claude")
            })
            .join("claude_desktop_config.json"),
        "windows" => std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
            .join("Claude/claude_desktop_config.json"),
        _ => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            })
            .join("Claude/claude_desktop_config.json"),
    };
    Ok(path)
}

fn merge_client_config(path: &std::path::Path, generated: &serde_json::Value) -> Result<()> {
    let mut document = if path.is_file() {
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path)?)
            .with_context(|| format!("{} is not valid JSON; left unchanged", path.display()))?
    } else {
        serde_json::json!({})
    };
    let root = document
        .as_object_mut()
        .context("client configuration root must be a JSON object")?;
    let servers = root
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("client mcpServers must be a JSON object")?;
    let generated_servers = generated
        .get("mcpServers")
        .and_then(serde_json::Value::as_object)
        .context("generated MCP configuration is invalid")?;
    for (name, server) in generated_servers {
        if servers.contains_key(name) {
            let replace = matches!(
                prompt(&format!(
                    "{name} already exists in {}; replace it? [y/N]",
                    path.display()
                ))?
                .trim()
                .to_lowercase()
                .as_str(),
                "y" | "yes"
            );
            if !replace {
                anyhow::bail!("left existing MCP server {name:?} unchanged");
            }
        }
        servers.insert(name.clone(), server.clone());
    }
    write_client_config(path, &document)
}

fn write_client_config(path: &std::path::Path, document: &serde_json::Value) -> Result<()> {
    if path.is_file() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or_else(|_| "0".to_string(), |elapsed| elapsed.as_secs().to_string());
        let mut backup_name = path.file_name().unwrap_or_default().to_os_string();
        backup_name.push(format!(".agentmesh-backup-{stamp}"));
        let backup = path.with_file_name(backup_name);
        std::fs::copy(path, &backup)
            .with_context(|| format!("could not back up {}", path.display()))?;
        println!("Backup of prior config: {}", backup.display());
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(document)?)
        .with_context(|| format!("could not write client config {}", path.display()))?;
    println!("MCP client configuration updated: {}", path.display());
    Ok(())
}

fn draw_diagnostics(output: &mut String) {
    output.push_str("  SYSTEM HEALTH\n\n");
    output.push_str("  ● Configuration file: config/agentmesh.yaml\n");
    output.push_str("  ● Workflow store: ");
    output.push_str(&agentmesh_teach::workflows_dir().map_or_else(
        |_| "not available".into(),
        |path| path.display().to_string(),
    ));
    output.push_str("\n  ● Capability catalog: ");
    output.push_str(&capability_cli::catalog_directory().display().to_string());
    output.push_str("\n\n  Press Enter to run the full doctor checks.\n");
}

fn draw_help(output: &mut String) {
    output.push_str("  KEYBOARD SHORTCUTS\n\n");
    output.push_str("  ↑ / ↓        Move through lists\n");
    output.push_str("  Enter        Open a view or selected item\n");
    output.push_str("  Esc / q      Go back or close the console\n");
    output.push_str("  /            Search capability catalog\n");
    output.push_str("  c            Clear capability search\n");
    output.push_str("  i            Install a capability package\n");
    output.push_str("  t            Record a browser session into a workflow\n");
    output.push_str("  c            Create or run a composite MCP tool from its menu\n");
    output.push_str("  Login Sessions lets you save logins and choose one when recording\n");
    output.push_str("  r            Run the selected workflow\n");
    output.push_str("  m            Create MCP server and connect an AI client\n");
    output.push_str("\n  Existing subcommands remain available for automation and scripts.\n");
    output.push_str("  Start the interactive console with: agentmesh\n");
}

fn draw_inventory_bars(
    workflows: usize,
    capabilities: usize,
    providers: usize,
    output: &mut String,
) {
    let maximum = workflows.max(capabilities).max(providers).max(1);
    for (label, value) in [
        ("Workflows", workflows),
        ("Capabilities", capabilities),
        ("MCP providers", providers),
    ] {
        let width = value
            .saturating_mul(24)
            .checked_div(maximum)
            .unwrap_or(0)
            .min(24);
        output.push_str(&format!("  {label:<16} {} {}\n", "█".repeat(width), value));
    }
}

fn workflow_summaries() -> Result<Vec<agentmesh_teach::WorkflowSummary>> {
    Ok(agentmesh_teach::list_workflows()?)
}

fn configured_upstreams() -> Result<Vec<agentmesh_config::UpstreamConfig>> {
    let config = agentmesh_config::Config::from_path("config/agentmesh.yaml")?;
    Ok(config
        .gateway
        .effective_upstreams()
        .into_iter()
        .cloned()
        .collect())
}

fn run_selected_workflow(app: &App) -> Result<()> {
    let id = app.selected_workflow.clone().or_else(|| {
        workflow_summaries()
            .ok()?
            .get(app.selected)
            .map(|summary| summary.id.clone())
    });
    let Some(id) = id else {
        app_notice("No workflow is selected.");
        return Ok(());
    };
    let workflow = agentmesh_teach::load_workflow(&id)?;
    let mut inputs = Vec::new();
    for (name, definition) in &workflow.inputs {
        let default = definition
            .default
            .as_ref()
            .map_or_else(String::new, |value| {
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned)
            });
        let label = if definition.is_required() {
            format!("{name} (required)")
        } else if default.is_empty() {
            format!("{name} (optional; empty to skip)")
        } else {
            format!("{name} [default: {default}]")
        };
        let value = prompt(&label)?;
        if !value.is_empty() {
            inputs.push(format!("{name}={value}"));
        } else if definition.is_required() {
            anyhow::bail!("required workflow input {name:?} was empty");
        } else if !default.is_empty() {
            inputs.push(format!("{name}={default}"));
        }
    }
    let profiles = session_profiles();
    let session = if profiles.is_empty() {
        None
    } else {
        println!("Saved login profiles:");
        for (index, (profile, active, _)) in profiles.iter().enumerate() {
            println!(
                "  {}. {}{}",
                index + 1,
                profile,
                if *active {
                    " (login data found)"
                } else {
                    " (login may be missing)"
                }
            );
        }
        let selection = prompt("Login profile to use (empty for temporary/no login)")?;
        if selection.trim().is_empty() {
            None
        } else {
            profiles
                .iter()
                .find(|item| item.0 == selection)
                .map(|item| item.0.clone())
                .or_else(|| {
                    selection
                        .parse::<usize>()
                        .ok()
                        .and_then(|number| number.checked_sub(1))
                        .and_then(|index| profiles.get(index).map(|item| item.0.clone()))
                })
        }
    };
    if !profiles.is_empty() && session.is_none() {
        let selection = prompt("No profile selected. Run with no login? [Y/n]")?;
        if matches!(selection.trim().to_lowercase().as_str(), "n" | "no") {
            return Ok(());
        }
    }
    let headed = matches!(
        prompt("Show browser while running? [y/N]")?
            .trim()
            .to_lowercase()
            .as_str(),
        "y" | "yes"
    );
    run_action(&format!("Run workflow {id}"), || {
        crate::teach_flow::workflows_run(
            &id,
            &crate::teach_flow::RunOptions {
                inputs,
                dry_run: false,
                yes: false,
                session,
                headed,
            },
        )
    })
}

fn install_package_from_path(app: &mut App) -> Result<()> {
    let source = prompt("Package file path")?;
    if source.trim().is_empty() {
        return Ok(());
    }
    let action = CapabilityAction::Install {
        file: PathBuf::from(source),
        catalog: app.catalog_root.clone(),
    };
    run_action("Install capability", || capability_cli::run(action))
}

fn run_action(label: &str, action: impl FnOnce() -> Result<()>) -> Result<()> {
    println!("\n── {label} ──\n");
    match action() {
        Ok(()) => println!("\n✓ Done"),
        Err(error) => println!("\n✗ {error:#}"),
    }
    pause_for_action();
    Ok(())
}

fn pause_for_action() {
    print!("\nPress Enter to return to the console...");
    let _ = io::stdout().flush();
    let mut line = String::new();
    let _ = io::stdin().read_line(&mut line);
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    io::stdout().flush()?;
    let mut value = String::new();
    io::stdin().read_line(&mut value)?;
    Ok(value.trim_end_matches(['\r', '\n']).to_owned())
}

fn app_notice(_message: &str) {
    println!("{_message}");
}

struct TerminalGuard {
    #[cfg(unix)]
    saved_state: String,
}

impl TerminalGuard {
    fn enter() -> Result<Self> {
        #[cfg(unix)]
        let saved_state = {
            let state = ProcessCommand::new("stty")
                .arg("-g")
                .stdin(Stdio::inherit())
                .output()
                .context("could not read terminal settings")?;
            if !state.status.success() {
                anyhow::bail!("stty could not inspect this terminal");
            }
            let saved = String::from_utf8_lossy(&state.stdout).trim().to_owned();
            let status = ProcessCommand::new("stty")
                .args(["-echo", "-icanon", "min", "0", "time", "2"])
                .stdin(Stdio::inherit())
                .status()
                .context("could not enable interactive terminal mode")?;
            if !status.success() {
                anyhow::bail!("stty could not enable interactive terminal mode");
            }
            saved
        };
        print!("\x1b[?1049h\x1b[?25l");
        io::stdout().flush()?;
        Ok(Self {
            #[cfg(unix)]
            saved_state,
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        print!("\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = io::stdout().flush();
        #[cfg(unix)]
        {
            let _ = ProcessCommand::new("stty")
                .arg(&self.saved_state)
                .stdin(Stdio::inherit())
                .status();
        }
    }
}
