use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::Context;
use brush_parser::ast::{Command, CompoundCommand, CompoundList, FunctionDefinition, Program};
use brush_parser::{Parser, ParserOptions, SourceInfo};

pub struct TestFile {
    pub tests: Vec<TestCase>,
    pub functions: Vec<FunctionDefinition>,
}

pub struct TestCase {
    pub file: PathBuf,
    pub name: String,
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
        })
        .collect();

    Ok(TestFile { tests, functions })
}

pub(crate) fn extract_functions(program: &Program) -> Vec<FunctionDefinition> {
    let mut functions = Vec::new();
    for complete_command in &program.complete_commands {
        extract_from_compound_list(complete_command, &mut functions);
    }
    functions
}

fn extract_from_compound_list(list: &CompoundList, functions: &mut Vec<FunctionDefinition>) {
    for item in &list.0 {
        let and_or = &item.0;
        extract_from_pipeline(&and_or.first, functions);
        for additional in &and_or.additional {
            let pipeline = match additional {
                brush_parser::ast::AndOr::And(p) | brush_parser::ast::AndOr::Or(p) => p,
            };
            extract_from_pipeline(pipeline, functions);
        }
    }
}

fn extract_from_pipeline(
    pipeline: &brush_parser::ast::Pipeline,
    functions: &mut Vec<FunctionDefinition>,
) {
    for cmd in &pipeline.seq {
        match cmd {
            Command::Function(func) => functions.push(func.clone()),
            Command::Compound(compound, _) => extract_from_compound(compound, functions),
            Command::Simple(_) | Command::ExtendedTest(_) => {}
        }
    }
}

fn extract_from_compound(compound: &CompoundCommand, functions: &mut Vec<FunctionDefinition>) {
    match compound {
        CompoundCommand::BraceGroup(b) => extract_from_compound_list(&b.list, functions),
        CompoundCommand::Subshell(s) => extract_from_compound_list(&s.list, functions),
        CompoundCommand::ForClause(f) => extract_from_compound_list(&f.body.list, functions),
        CompoundCommand::ArithmeticForClause(a) => {
            extract_from_compound_list(&a.body.list, functions)
        }
        CompoundCommand::WhileClause(w) => {
            extract_from_compound_list(&w.0, functions);
            extract_from_compound_list(&w.1.list, functions);
        }
        CompoundCommand::IfClause(i) => {
            extract_from_compound_list(&i.condition, functions);
            extract_from_compound_list(&i.then, functions);
            if let Some(elses) = &i.elses {
                for else_clause in elses {
                    if let Some(cond) = &else_clause.condition {
                        extract_from_compound_list(cond, functions);
                    }
                    extract_from_compound_list(&else_clause.body, functions);
                }
            }
        }
        CompoundCommand::CaseClause(c) => {
            for case_item in &c.cases {
                if let Some(cmd) = &case_item.cmd {
                    extract_from_compound_list(cmd, functions);
                }
            }
        }
        CompoundCommand::UntilClause(w) => {
            extract_from_compound_list(&w.0, functions);
            extract_from_compound_list(&w.1.list, functions);
        }
        CompoundCommand::Arithmetic(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_script(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn parse_file_with_test_functions() {
        let tmp = TempDir::new().unwrap();
        let path = write_script(
            tmp.path(),
            "example.test",
            "test_hello() {\n  echo hello\n}\n\ntest_world() {\n  echo world\n}\n",
        );

        let result = parse_test_file(&path).unwrap();
        assert_eq!(result.tests.len(), 2);
        assert_eq!(result.tests[0].name, "test_hello");
        assert_eq!(result.tests[1].name, "test_world");
        assert_eq!(result.functions.len(), 2);
    }

    #[test]
    fn parse_file_with_helpers_and_tests() {
        let tmp = TempDir::new().unwrap();
        let path = write_script(
            tmp.path(),
            "example.test",
            "setup() {\n  mkdir -p /tmp/test\n}\n\ntest_foo() {\n  setup\n  echo foo\n}\n",
        );

        let result = parse_test_file(&path).unwrap();
        assert_eq!(result.tests.len(), 1);
        assert_eq!(result.tests[0].name, "test_foo");
        assert_eq!(result.functions.len(), 2);
    }

    #[test]
    fn parse_file_with_no_functions() {
        let tmp = TempDir::new().unwrap();
        let path = write_script(tmp.path(), "plain.sh", "echo hello\necho world\n");

        let result = parse_test_file(&path).unwrap();
        assert!(result.tests.is_empty());
        assert!(result.functions.is_empty());
    }

    #[test]
    fn parse_file_with_no_test_functions() {
        let tmp = TempDir::new().unwrap();
        let path = write_script(
            tmp.path(),
            "helpers.sh",
            "helper_one() {\n  echo one\n}\n\nhelper_two() {\n  echo two\n}\n",
        );

        let result = parse_test_file(&path).unwrap();
        assert!(result.tests.is_empty());
        assert_eq!(result.functions.len(), 2);
    }

    #[test]
    fn parse_file_preserves_path() {
        let tmp = TempDir::new().unwrap();
        let path = write_script(tmp.path(), "my.test", "test_a() {\n  true\n}\n");

        let result = parse_test_file(&path).unwrap();
        assert_eq!(result.tests[0].file, path);
    }

    #[test]
    fn parse_nonexistent_file_errors() {
        let result = parse_test_file(Path::new("/nonexistent/file.test"));
        assert!(result.is_err());
    }

    #[test]
    fn test_prefix_is_strict() {
        let tmp = TempDir::new().unwrap();
        let path = write_script(
            tmp.path(),
            "example.test",
            "testing() {\n  echo nope\n}\n\ntest_real() {\n  true\n}\n\nmy_test() {\n  echo no\n}\n",
        );

        let result = parse_test_file(&path).unwrap();
        // "testing" starts with "test" so it is a test; "my_test" does not start with "test"
        let names: Vec<&str> = result.tests.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"testing"));
        assert!(names.contains(&"test_real"));
        assert!(!names.contains(&"my_test"));
    }

    #[test]
    fn extract_functions_from_nested_blocks() {
        let tmp = TempDir::new().unwrap();
        // Function defined inside an if block
        let path = write_script(
            tmp.path(),
            "nested.sh",
            "if true; then\n  test_inside_if() {\n    true\n  }\nfi\n\nfor x in 1; do\n  test_inside_for() {\n    true\n  }\ndone\n",
        );

        let result = parse_test_file(&path).unwrap();
        let names: Vec<&str> = result.tests.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"test_inside_if"));
        assert!(names.contains(&"test_inside_for"));
    }

    #[test]
    fn parse_empty_file() {
        let tmp = TempDir::new().unwrap();
        let path = write_script(tmp.path(), "empty.sh", "");

        let result = parse_test_file(&path).unwrap();
        assert!(result.tests.is_empty());
        assert!(result.functions.is_empty());
    }
}
