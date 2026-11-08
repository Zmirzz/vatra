use std::path::Path;

use serde_yaml_ng::{Mapping, Value};

use crate::FormatError;

/// A markdown file split into its YAML frontmatter and the body that follows it.
pub(crate) struct Document {
    /// `None` when the file has no frontmatter.
    pub meta: Option<Mapping>,
    pub body: String,
}

/// Splits a file at its leading `---` block. The block must start on the first line. The body has its
/// line endings as `\n`.
pub(crate) fn split(text: &str, file: &Path) -> Result<Document, FormatError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let opens = lines.next().is_some_and(|line| line.trim_end() == "---");
    if !opens {
        return Ok(Document {
            meta: None,
            body: text.replace("\r\n", "\n"),
        });
    }

    let mut yaml = String::new();
    let mut consumed = text.find('\n').map_or(text.len(), |end| end + 1);
    for line in lines {
        consumed += line.len();
        if line.trim_end() == "---" {
            let meta = parse_meta(&yaml, file)?;
            return Ok(Document {
                meta: Some(meta),
                body: text[consumed..].replace("\r\n", "\n"),
            });
        }
        yaml.push_str(line);
    }
    Err(FormatError::Frontmatter {
        path: file.to_path_buf(),
        message: "no closing `---` line".to_string(),
    })
}

fn parse_meta(yaml: &str, file: &Path) -> Result<Mapping, FormatError> {
    let value: Value = serde_yaml_ng::from_str(yaml).map_err(|source| FormatError::Yaml {
        path: file.to_path_buf(),
        source,
    })?;
    match value {
        Value::Mapping(meta) => Ok(meta),
        Value::Null => Ok(Mapping::new()),
        _ => Err(FormatError::Frontmatter {
            path: file.to_path_buf(),
            message: "the frontmatter must be a mapping of keys to values".to_string(),
        }),
    }
}
