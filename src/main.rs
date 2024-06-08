use std::collections::HashSet;
use std::env;
use base64::{Engine as _, engine::{general_purpose}};
use std::io::{Write};
use zip::write::{FileOptions, ZipWriter};
use std::io::Cursor;
use dotenv::dotenv;
use regex::Regex;
use reqwest::blocking::Client;

struct PathsMap {
    download: String,
    script: String,
    project: String,
    common_helpers: String
}

struct FileObject {
    script_name: String,
    contents: Vec<String>,
}

fn gather_args() -> Vec<String> {
    let args: Vec<String> = env::args().collect();

    if args.len() <= 2 && args.len() >= 4 {
        eprintln!("Usage: {} <group_name> <script_name>", args[0]);
        std::process::exit(1);
    }

    args
}

fn fetch_file_content(url: &str) -> Vec<String> {
    let client = Client::new();
    let response = client.get(url).send().unwrap();
    let content = response.text().unwrap();
    content.lines().map(|line| line.to_string()).collect()
}

fn describe_paths(group_name: &String, script_name: &String) -> PathsMap {
    let root_directory = env::var("ROOT_DIRECTORY").expect("ROOT_DIRECTORY not set");

    PathsMap {
        download: format!("{}/{}/{}/download.py", root_directory, group_name, script_name),
        script: format!("{}/{}/{}/script.py", root_directory, group_name, script_name),
        common_helpers: format!("{}/common/helpers.py", root_directory),
        project: format!("{}", root_directory)
    }
}

fn build_bundle(paths: &PathsMap) -> Vec<String> {
    let mut bundled_output_lines = Vec::new();

    let entry_file = fetch_file_content(&paths.download);

    for line in entry_file {
        if !line.starts_with("import") && !line.starts_with("from") {
            bundled_output_lines.push(line);
            continue;
        }

        if line.contains("common.helpers") {
            let lines = bundle_common_import_lines(&line, &paths.common_helpers);
            bundled_output_lines.extend(lines);
        } else if line.contains(".script") {
            let lines = bundle_script_import_lines(&line, &paths);
            bundled_output_lines.extend(lines);
        }
    }

    bundled_output_lines
}

fn extract_function_names_from_import(line: &String) -> HashSet<String> {
    let import_re = Regex::new(r"from \S+ import (.+)").unwrap();

    let mut functions_to_include = HashSet::new();

    if let Some(caps) = import_re.captures(line) {
        let functions_str = &caps[1];

        for function in functions_str.split(',').map(|s| s.trim()) {
            functions_to_include.insert(function.to_string());
        }
    }

    functions_to_include
}

fn bundle_common_import_lines(line: &String, common_helpers: &String) -> Vec<String> {
    let functions_to_include = extract_function_names_from_import(line);

    let file = fetch_file_content(common_helpers);
    let mut output_lines = Vec::new();
    let mut capture = false;
    let mut indent_level = None;

    for line in file {
        if let Some(caps) = Regex::new(r"^def (\w+)\(").unwrap().captures(&line) {
            let func_name = &caps[1];
            if functions_to_include.contains(func_name) {
                capture = true;
                indent_level = Some(line.find(|c: char| !c.is_whitespace()).unwrap_or(0));
            } else {
                capture = false;
            }
        }

        if let Some(caps) = Regex::new(r"^([A-Z_]+)\s*=").unwrap().captures(&line) {
            let var_name = &caps[1];
            if functions_to_include.contains(var_name) {
                capture = true;
                indent_level = Some(line.find(|c: char| !c.is_whitespace()).unwrap_or(0));
            } else {
                capture = false;
            }
        }

        if capture {
            output_lines.push(line.clone());
            let current_indent = line.find(|c: char| !c.is_whitespace()).unwrap_or(0);
            if indent_level.is_some() && current_indent <= indent_level.unwrap() && line.trim().is_empty() {
                capture = false;
            }
        }
    }

    output_lines
}

fn bundle_script_import_lines(_line: &String, paths: &PathsMap) -> Vec<String> {
    let mut output_lines = Vec::new();
    let file = fetch_file_content(&paths.script);

    for script_line in file {

        if script_line.contains("common.helpers") {
            let helper_lines = bundle_common_import_lines(&script_line, &paths.common_helpers);
            output_lines.extend(helper_lines);
        } else if script_line.contains("from") && script_line.contains("import") {
            if let Some(adjacent_path) = resolve_adjacent_script_path(&script_line, paths) {
                let adjacent_lines = bundle_adjacent_script_import_lines(&script_line, &adjacent_path);
                output_lines.extend(adjacent_lines);
            }
        } else {
            output_lines.push(script_line);
        }
    }

    output_lines
}

fn resolve_adjacent_script_path(line: &str, paths: &PathsMap) -> Option<String> {
    let import_section = line.split_whitespace().nth(1)?;

    let mut parts = import_section.split('.');

    let group_name = parts.next()?;
    let script_name = parts.next()?;
    let file_name = parts.next()?;

    Some(format!("{}/{}/{}/{}.py", paths.project, group_name, script_name, file_name))
}

fn bundle_adjacent_script_import_lines(_line: &String, script_path: &String) -> Vec<String> {
    let mut output_lines = Vec::new();
    let file = fetch_file_content(script_path);

    for script_line in file {

        if script_line.contains("common.helpers") {
            let helper_lines = bundle_common_import_lines(&script_line, script_path);
            output_lines.extend(helper_lines);
        }

        output_lines.push(script_line);
    }

    output_lines
}

fn create_zip(files: Vec<FileObject>) -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());

    let mut zip = ZipWriter::new(&mut buffer);

    let options: FileOptions<()> = FileOptions::default()
        .compression_method(zip::CompressionMethod::Stored)
        .unix_permissions(0o755);

    for file in files {
        zip.start_file(format!("{}.py", file.script_name.as_str()), options).unwrap();

        let file_contents = file.contents.join("\n");
        zip.write_all(file_contents.as_bytes()).unwrap();
    }

    zip.finish().unwrap();

    buffer.into_inner()
}

fn main() {
    dotenv().ok();

    let args = gather_args();

    let mut files = Vec::new();

    for script_name in args[2].split(',').map(|s| s.trim()) {
        let paths = describe_paths(&args[1], &script_name.to_string());

        let bundled_output_lines: Vec<String> = build_bundle(&paths);

        if args.len() == 4 && args[3] == "DEV" {
            for demo_line in &bundled_output_lines {
                println!("{}", demo_line);
            }
        }

        files.push(FileObject {
            script_name: script_name.to_string(),
            contents: bundled_output_lines,
        })
    }

    let zip_content = create_zip(files);

    println!("{}", general_purpose::STANDARD.encode(&zip_content));
}
