use clap::Parser;
use std::collections::HashMap;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::{env, fs::read_dir};

use semver::Version;
use serde_json::{Value, from_str};

const BUILTIN_CHIPS: [&str; 18] = [
    // Merge / Split
    "4-1BIT",
    "1-4BIT",
    "4-8BIT",
    "8-4BIT",
    "1-8BIT",
    "8-1BIT",
    // Display
    "LED",
    "7-SEGMENT",
    "RGB DISPLAY",
    "DOT DISPLAY",
    // Memory
    "ROM 256x16",
    // Basic
    "CLOCK",
    "PULSE",
    "KEY",
    "3-STATE BUFFER",
    // Bus
    "BUS-1",
    "BUS-4",
    "BUS-8",
];

macro_rules! path {
    ($($segment:expr),+ $(,)?) => {{
        let mut path = std::path::PathBuf::new();
        $( path.push($segment); )+
        path
    }};
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Project {
    name: String,
    path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Chip {
    NAND_count: usize,
    checked: bool,
}

impl Default for Chip {
    fn default() -> Self {
        Chip {
            NAND_count: 0,
            checked: false,
        }
    }
}

struct ProjectManager {
    pub game_dir: PathBuf,
    pub projects: Vec<Project>,
}

impl ProjectManager {
    fn new(game_dir: Option<PathBuf>) -> Self {
        let mut sim = Self {
            game_dir: game_dir.unwrap_or_else(|| PathBuf::new()),
            projects: Vec::new(),
        };
        #[cfg(target_os = "windows")]
        {
            sim.game_dir = path!(
                env::var("USERPROFILE").unwrap(),
                "AppData",
                "LocalLow",
                "SebastianLague",
                "Digital-Logic-Sim"
            );
        }
        #[cfg(target_os = "linux")]
        {
            sim.game_dir = path!(
                env::var("HOME").unwrap(),
                ".config",
                "unity3d",
                "SebastianLague",
                "Digital-Logic-Sim"
            );
        }
        // Might not work if app is not fully installed
        #[cfg(target_os = "macos")]
        {
            sim.game_dir = path!(
                env::var("HOME").unwrap(),
                "Library",
                "Application Support",
                "unity3d",
                "SebastianLague",
                "Digital-Logic-Sim"
            );
        }

        let projects_path = path!(&sim.game_dir, "Projects");

        let projects: Vec<Project> = read_dir(&projects_path)
            .unwrap_or_else(|e| panic!("Failed to read project directory: {}", e))
            .filter_map(|entry| match entry {
                Ok(e) => Some(Project {
                    name: e.file_name().to_string_lossy().into_owned(),
                    path: e.path(),
                }),
                Err(_) => None,
            })
            .collect();

        sim.projects = projects.into_iter().filter(|p| p.path.is_dir()).collect();
        sim
    }

    fn list_projects(&self) {
        println!("Choose a DLS Project to NAND scan:");
        let mut longest_name = 3;
        for (i, project) in self.projects.iter().enumerate() {
            longest_name = longest_name.max(project.name.len());
            println!("  {}.) {}", i + 1, project.name);
        }
        println!("{}", "-".repeat(longest_name + 7));
        println!("  {}.) All", self.projects.len() + 1);
    }

    fn prompt_and_scan(&self) {
        self.list_projects();
        let mut input = String::new();
        print!("Enter your choice: ");
        io::stdout().flush().unwrap();
        io::stdin().lock().read_line(&mut input).unwrap();

        match input.trim().parse::<usize>() {
            Ok(n) if n == self.projects.len() + 1 => {
                println!("Scanning all projects...");
                for p in &self.projects {
                    if let Some(result) = self.scan_project(p) {
                        println!("{}", result);
                    }
                }
            }
            Ok(n) if n > 0 && n <= self.projects.len() => {
                if let Some(result) = self.scan_project(&self.projects[n - 1]) {
                    println!("{}", result);
                }
            }
            _ => {
                eprintln!("Invalid choice.");
            }
        }
    }

    fn add_default_chips(&self, chip_map: &mut HashMap<String, Chip>) {
        chip_map.insert(
            "NAND".to_string(),
            Chip {
                NAND_count: 1,
                checked: true,
            },
        );

        for name in BUILTIN_CHIPS.iter() {
            chip_map.insert(
                name.to_string(),
                Chip {
                    NAND_count: 0,
                    checked: true,
                },
            );
        }
    }

    fn scan_project<'a>(&self, project: &'a Project) -> Option<ProjectScanResult<'a>> {
        println!("Scanning project: {}", project.name);
        let meta_path = path!(&project.path, "ProjectDescription.json");
        if !meta_path.exists() {
            eprintln!(
                "[DEBUG] Skipping {}: missing ProjectDescription.json",
                project.name
            );
            return None;
        }

        let contents = match std::fs::read_to_string(meta_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[DEBUG] Failed to read metadata for {}: {}",
                    project.name, e
                );
                return None;
            }
        };

        let metadata: Value = match from_str(&contents) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[DEBUG] Failed to parse JSON for {}: {}", project.name, e);
                return None;
            }
        };

        let ecv = metadata["DLSVersion_EarliestCompatible"]
            .as_str()
            .and_then(|s| Version::parse(s).ok());

        if let Some(version) = ecv {
            if version > Version::parse("2.1.5").unwrap() {
                eprintln!(
                    "[DEBUG] Skipping {}: incompatible version {} > 2.1.5",
                    project.name, version
                );
                return None;
            }
        } else {
            eprintln!(
                "[DEBUG] Skipping {}: missing or invalid DLSVersion_EarliestCompatible",
                project.name
            );
            return None;
        }

        let binding = Vec::new();
        let chips = metadata["AllCustomChipNames"]
            .as_array()
            .unwrap_or(&binding)
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>();

        if chips.is_empty() {
            eprintln!("[DEBUG] Skipping {}: no custom chips found", project.name);
            return None;
        }

        let mut chip_map: HashMap<String, Chip> = HashMap::new();

        self.add_default_chips(&mut chip_map);

        for name in &chips {
            chip_map.entry(name.to_string()).or_default();
        }

        for name in &chips {
            if let Err(e) = self.check_chip(name, &mut chip_map, &project.path) {
                eprintln!(
                    "[DEBUG] Error scanning chip {} in {}: {}",
                    name, project.name, e
                );
            }
        }

        let total_nand = chip_map.values().map(|c| c.NAND_count).sum();

        Some(ProjectScanResult {
            project,
            chip_map,
            total_nand,
        })
    }

    fn check_chip(
        &self,
        chip: &str,
        chip_map: &mut HashMap<String, Chip>,
        base_path: &PathBuf,
    ) -> Result<(), String> {
        if let Some(existing) = chip_map.get(chip) {
            if existing.checked {
                return Ok(());
            }
        }

        let chip_path = path!(base_path, "Chips", format!("{}.json", chip));
        if !chip_path.exists() {
            return Err(format!("Chip file not found: {}", chip));
        }

        let content = std::fs::read_to_string(&chip_path)
            .map_err(|_| format!("Failed to read chip file for {}", chip))?;
        let data: Value =
            from_str(&content).map_err(|_| format!("Failed to parse JSON for {}", chip))?;

        let subchips = data["SubChips"]
            .as_array()
            .ok_or_else(|| format!("SubChips missing or not array for {}", chip))?;

        let mut nand_total = 0;

        for subchip in subchips {
            let name = subchip
                .get("Name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("SubChip entry missing Name in {}", chip))?;

            if !chip_map.contains_key(name) {
                chip_map.insert(name.to_string(), Chip::default());
            }

            if !chip_map.get(name).unwrap().checked {
                self.check_chip(name, chip_map, base_path)?;
            }

            nand_total += chip_map.get(name).unwrap().NAND_count;
        }

        let entry = chip_map.get_mut(chip).unwrap();
        entry.NAND_count = nand_total;
        entry.checked = true;

        Ok(())
    }
}

struct ProjectScanResult<'a> {
    project: &'a Project,
    chip_map: HashMap<String, Chip>,
    total_nand: usize,
}

impl<'a> fmt::Display for ProjectScanResult<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let filtered_chip_map: HashMap<_, _> = self
            .chip_map
            .iter()
            .filter(|(k, _)| !BUILTIN_CHIPS.contains(&k.as_str()))
            .collect();
        let longest_name = filtered_chip_map.keys().map(|s| s.len()).max().unwrap_or(0);
        let most_NAND = filtered_chip_map
            .values()
            .map(|c| c.NAND_count)
            .max()
            .unwrap_or(0)
            .to_string()
            .len();
        writeln!(f, "Project: {}", self.project.name)?;
        writeln!(f, "Path: {}", self.project.path.display())?;
        writeln!(f, "Total NAND: {}", self.total_nand)?;

        let chip_count = filtered_chip_map.len();
        let avg_nand = if chip_count > 0 {
            self.total_nand as f64 / chip_count as f64
        } else {
            0.0
        };

        writeln!(f, "Average NAND per chip: {:.1}", avg_nand)?;
        writeln!(f, "{}", "-".repeat(40))?;

        writeln!(f, "Chips:")?;

        let mut chips: Vec<_> = filtered_chip_map.iter().collect();
        chips.sort_by_key(|(_, c)| usize::MAX - c.NAND_count);

        for (name, chip) in chips {
            if BUILTIN_CHIPS.contains(&name.as_str()) {
                continue;
            }
            let above_avg = if avg_nand > 0.0 {
                (chip.NAND_count as f64 - avg_nand) / avg_nand * 100.0
            } else {
                0.0
            };
            let total_percent = if self.total_nand > 0 {
                (chip.NAND_count as f64 / self.total_nand as f64) * 100.0
            } else {
                0.0
            };
            writeln!(
                f,
                "{}:{} {},{} {:+.1}%, {:.1}%",
                name,
                " ".repeat(longest_name - name.len()),
                chip.NAND_count,
                " ".repeat(most_NAND - chip.NAND_count.to_string().len()),
                above_avg,
                total_percent
            )?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Parser)]
struct Args {
    /// optional Path to the game directory
    game_dir: Option<PathBuf>,
}

fn main() {
    let args = Args::parse();
    let manager = ProjectManager::new(args.game_dir);
    manager.prompt_and_scan();
}
