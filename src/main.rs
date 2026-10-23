//! lobby: one library of every game on the computer, whichever store or launcher (or none)
//! put it there.

mod library;
mod sources;

use library::{Game, Kind};
use std::process::ExitCode;

const USAGE: &str = "usage: lobby scan [--json]
  Lists every game found on this computer and where each was found.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("scan") => {
            let Some(home) = sources::Home::find() else {
                eprintln!("lobby: no home folder");
                return ExitCode::FAILURE;
            };
            let found = sources::scan(&home);
            let games = library::merge(found.entries, &found.roots);
            if args.iter().any(|a| a == "--json") {
                println!("{}", serde_json::to_string_pretty(&json(&games)).unwrap());
            } else {
                print(&games);
            }
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn where_found(g: &Game) -> String {
    g.sources().iter().map(|s| s.label()).collect::<Vec<_>>().join(", ")
}

fn place(g: &Game) -> String {
    g.program().or(g.folder()).map(|p| p.display().to_string()).unwrap_or_default()
}

fn print(games: &[Game]) {
    let sections: [(&str, Box<dyn Fn(&Game) -> bool>); 5] = [
        ("Games", Box::new(|g: &Game| g.kind == Kind::Game && !g.missing && !g.unknown_to_launchers())),
        ("Games no launcher knows about", Box::new(|g: &Game| g.kind == Kind::Game && !g.missing && g.unknown_to_launchers())),
        ("Downloaded, not installed yet", Box::new(|g: &Game| g.kind == Kind::Download && !g.missing)),
        ("Store apps", Box::new(|g: &Game| g.kind == Kind::StoreApp && !g.missing)),
        ("Listed somewhere, but not on disk", Box::new(|g: &Game| g.missing)),
    ];
    for (title, wanted) in sections {
        let list: Vec<&Game> = games.iter().filter(|g| wanted(g)).collect();
        if list.is_empty() {
            continue;
        }
        println!("\n{title} ({})", list.len());
        let width = list.iter().map(|g| g.name.chars().count()).max().unwrap_or(0).min(40);
        for g in list {
            let name: String = g.name.chars().take(40).collect();
            println!("  {name:width$}  {}", where_found(g));
            println!("  {:width$}  {}", "", place(g));
        }
    }
}

fn json(games: &[Game]) -> serde_json::Value {
    let path = |p: &Option<std::path::PathBuf>| p.as_ref().map(|p| p.display().to_string());
    serde_json::Value::Array(
        games
            .iter()
            .map(|g| {
                serde_json::json!({
                    "name": g.name,
                    "kind": format!("{:?}", g.kind),
                    "missing": g.missing,
                    "found": g.entries.iter().map(|e| serde_json::json!({
                        "source": e.source.label(),
                        "name": e.name,
                        "folder": path(&e.folder),
                        "program": path(&e.program),
                        "prefix": path(&e.prefix),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}
