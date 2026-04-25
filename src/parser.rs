use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::Context;
use brush_parser::ast::{Command, FunctionDefinition, Program};
use brush_parser::{Parser, ParserOptions, SourceInfo};

pub struct TestFile {
    pub path: PathBuf,
    pub tests: Vec<TestCase>,
    pub functions: Vec<FunctionDefinition>,
}

pub struct TestCase {
    pub file: PathBuf,
    pub name: String,
    pub definition: FunctionDefinition,
}

pub fn parse_test_file(path: &Path) -> anyhow::Result<TestFile> {
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let reader = BufReader::new(contents.as_bytes());
    let options = ParserOptions::default();
    let source_info = SourceInfo {
        source: path.display().to_string(),
    };
    let mut parser = Parser::new(reader, &options, &source_info);
    let program = parser
        .parse_program()
        .map_err(|e| anyhow::anyhow!("parse error in {}: {e}", path.display()))?;

    let functions = extract_functions(&program);
    let tests = functions
        .iter()
        .filter(|f| f.fname.value.starts_with("test"))
        .map(|f| TestCase {
            file: path.to_path_buf(),
            name: f.fname.value.clone(),
            definition: f.clone(),
        })
        .collect();

    Ok(TestFile {
        path: path.to_path_buf(),
        tests,
        functions,
    })
}

fn extract_functions(program: &Program) -> Vec<FunctionDefinition> {
    let mut functions = Vec::new();
    for complete_command in &program.complete_commands {
        for item in &complete_command.0 {
            let and_or = &item.0;
            for cmd in &and_or.first.seq {
                if let Command::Function(func) = cmd {
                    functions.push(func.clone());
                }
            }
            for additional in &and_or.additional {
                let pipeline = match additional {
                    brush_parser::ast::AndOr::And(p) | brush_parser::ast::AndOr::Or(p) => p,
                };
                for cmd in &pipeline.seq {
                    if let Command::Function(func) = cmd {
                        functions.push(func.clone());
                    }
                }
            }
        }
    }
    functions
}
