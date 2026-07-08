use bt_core::{InstructionDocument, Result};
use camino::{Utf8Path, Utf8PathBuf};
use std::collections::BTreeMap;
use std::fs;

#[derive(Clone, Debug)]
pub struct MarkdownInstructionResolver {
    core_prompt: InstructionDocument,
    provider_overlays: BTreeMap<String, InstructionDocument>,
    global_dir: Utf8PathBuf,
}

impl MarkdownInstructionResolver {
    pub fn new(global_dir: Utf8PathBuf, prompts_dir: Utf8PathBuf) -> Result<Self> {
        let core_prompt = load_required_markdown_file(&prompts_dir.join("00-core.md"))?;
        let provider_overlays = load_markdown_dir(&prompts_dir.join("provider"))?
            .into_iter()
            .map(|doc| {
                let provider = doc
                    .source
                    .rsplit('/')
                    .next()
                    .and_then(|name| name.strip_suffix(".md"))
                    .ok_or_else(|| {
                        bt_core::BelltowerError::Config(format!(
                            "invalid provider prompt asset source `{}`",
                            doc.source
                        ))
                    })?;
                Ok((provider.to_owned(), doc))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;

        Ok(Self {
            core_prompt,
            provider_overlays,
            global_dir,
        })
    }

    pub fn resolve(&self, project_root: Option<&Utf8Path>) -> Result<Vec<InstructionDocument>> {
        let mut docs = Vec::new();
        docs.extend(load_markdown_dir(&self.global_dir)?);

        if let Some(project_root) = project_root {
            docs.extend(load_markdown_dir(&project_root.join(".belltower/skills"))?);
        }

        docs.sort_by(|left, right| left.source.cmp(&right.source));
        Ok(docs)
    }
    #[must_use]
    pub fn core_prompt(&self) -> InstructionDocument {
        self.core_prompt.clone()
    }

    #[must_use]
    pub fn provider_overlay(&self, provider_family: &str) -> Option<InstructionDocument> {
        self.provider_overlays.get(provider_family).cloned()
    }
}

fn load_markdown_dir(path: &Utf8Path) -> Result<Vec<InstructionDocument>> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let mut entries = fs::read_dir(path)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = Utf8PathBuf::from_path_buf(entry.path()).ok()?;
            if path.extension() == Some("md") {
                Some(path)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    entries.sort();

    entries
        .into_iter()
        .map(|path| {
            let body = fs::read_to_string(&path)?;
            let title = path
                .file_stem()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "instruction".to_owned());
            Ok(InstructionDocument {
                source: path.to_string(),
                title,
                body,
            })
        })
        .collect()
}

fn load_required_markdown_file(path: &Utf8Path) -> Result<InstructionDocument> {
    if !path.exists() {
        return Err(bt_core::BelltowerError::Config(format!(
            "required prompt asset `{path}` is missing"
        )));
    }

    let body = fs::read_to_string(path)?;
    let title = path
        .file_stem()
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "instruction".to_owned());
    Ok(InstructionDocument {
        source: path.to_string(),
        title,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::MarkdownInstructionResolver;
    use camino::Utf8PathBuf;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("mkdir");
        }
        fs::write(path, content).expect("write");
    }

    #[test]
    fn resolver_loads_prompt_assets_and_instruction_dirs_deterministically() {
        let temp = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8");
        let global_dir = root.join("global");
        let prompts_dir = root.join("prompts");
        let project_root = root.join("project");

        write(prompts_dir.join("00-core.md").as_std_path(), "Core rules");
        write(
            prompts_dir
                .join("provider/openai-compatible.md")
                .as_std_path(),
            "Provider notes",
        );
        write(
            global_dir.join("20-global.md").as_std_path(),
            "Global guidance",
        );
        write(
            project_root
                .join(".belltower/skills/10-project.md")
                .as_std_path(),
            "Project guidance",
        );

        let resolver = MarkdownInstructionResolver::new(global_dir.clone(), prompts_dir.clone())
            .expect("resolver");
        let core = resolver.core_prompt();
        let overlay = resolver
            .provider_overlay("openai-compatible")
            .expect("overlay");
        let instructions = resolver.resolve(Some(&project_root)).expect("instructions");

        assert_eq!(core.body, "Core rules");
        assert_eq!(overlay.body, "Provider notes");
        assert_eq!(
            instructions
                .into_iter()
                .map(|doc| doc.title)
                .collect::<Vec<_>>(),
            vec!["20-global".to_owned(), "10-project".to_owned()]
        );
    }

    #[test]
    fn resolver_requires_core_prompt_asset() {
        let temp = TempDir::new().expect("tempdir");
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf()).expect("utf8");
        let error = MarkdownInstructionResolver::new(root.join("global"), root.join("prompts"))
            .expect_err("missing core");

        assert!(error.to_string().contains("00-core.md"));
    }
}
