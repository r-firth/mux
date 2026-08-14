use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use mux_acp::AgentSlashCommand;

const MAX_COMPLETIONS: usize = 7;
const MAX_INDEXED_FILES: usize = 30_000;
const MAX_REFERENCED_FILES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AgentCompletionKind {
    Command,
    Value,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentCompletion {
    pub kind: AgentCompletionKind,
    pub label: String,
    pub detail: String,
    pub description: String,
    pub replacement: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentCompletionMenu {
    pub items: Vec<AgentCompletion>,
    pub selected: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AgentCommandArgument {
    pub command: String,
    pub value: String,
    pub detail: String,
    pub description: String,
}

impl AgentCompletionMenu {
    pub(crate) fn select_relative(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = match delta.cmp(&0) {
            std::cmp::Ordering::Less => {
                self.selected.checked_sub(1).unwrap_or(self.items.len() - 1)
            }
            std::cmp::Ordering::Greater => (self.selected + 1) % self.items.len(),
            std::cmp::Ordering::Equal => self.selected,
        };
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandCandidate {
    name: String,
    description: String,
    source: CommandSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandSource {
    Mux,
    Agent,
}

const LOCAL_COMMANDS: &[(&str, &str)] = &[
    ("new", "Start a new agent session: /new [agent] [cwd]"),
    ("next", "Switch to the next agent session in this tab"),
    ("prev", "Switch to the previous agent session in this tab"),
    ("use", "Switch agent session by number or name"),
    ("end", "End the active agent session"),
    ("cancel", "Cancel the active agent turn"),
    ("context", "Choose terminal context: /context tab|none"),
    ("mode", "Change the active agent mode"),
    ("model", "Change the active agent model"),
    ("effort", "Change the active agent reasoning effort"),
    ("login", "Authenticate with the active agent"),
    ("allow", "Approve the pending permission request"),
    ("deny", "Reject the pending permission request"),
    ("expand", "Expand the latest tool or thinking detail"),
    ("collapse", "Collapse the latest tool or thinking detail"),
    (
        "help",
        "Show keyboard controls and available commands in the conversation",
    ),
];

#[derive(Default)]
pub(crate) struct AgentCompletionProvider {
    agent_commands: RefCell<Vec<AgentSlashCommand>>,
    command_arguments: RefCell<Vec<AgentCommandArgument>>,
    files: RefCell<FileCatalog>,
}

#[derive(Default)]
struct FileCatalog {
    root: Option<PathBuf>,
    index: AgentFileIndex,
}

#[derive(Default)]
pub(crate) struct AgentFileIndex {
    entries: Vec<IndexedFile>,
    lookup: HashMap<String, PathBuf>,
}

#[derive(Clone)]
struct IndexedFile {
    path: PathBuf,
    display: String,
    search: String,
    file_name: String,
    depth: usize,
}

impl AgentCompletionProvider {
    pub(crate) fn set_agent_commands(&self, commands: Vec<AgentSlashCommand>) {
        let mut current = self.agent_commands.borrow_mut();
        if *current != commands {
            *current = commands;
        }
    }

    pub(crate) fn set_command_arguments(&self, arguments: Vec<AgentCommandArgument>) {
        let mut current = self.command_arguments.borrow_mut();
        if *current != arguments {
            *current = arguments;
        }
    }

    pub(crate) fn set_file_index(&self, root: PathBuf, index: AgentFileIndex) {
        let mut catalog = self.files.borrow_mut();
        catalog.root = Some(root);
        catalog.index = index;
    }

    pub(crate) fn clear_file_index(&self) {
        let mut catalog = self.files.borrow_mut();
        *catalog = FileCatalog::default();
    }

    pub(crate) fn completions(&self, text: &str, cursor: usize) -> Vec<AgentCompletion> {
        if let Some(token) = slash_argument_completion(text, cursor) {
            let arguments = self.command_arguments.borrow();
            let query = token.query.to_lowercase();
            let mut candidates = arguments
                .iter()
                .filter(|argument| argument.command.eq_ignore_ascii_case(token.command))
                .filter_map(|argument| {
                    let rank = command_rank(&argument.value, &query);
                    (rank.0 < u8::MAX).then_some((rank, argument))
                })
                .collect::<Vec<_>>();
            if !query.is_empty() {
                candidates.sort_by(|(left_rank, left), (right_rank, right)| {
                    left_rank
                        .cmp(right_rank)
                        .then_with(|| left.value.cmp(&right.value))
                });
            }
            return candidates
                .into_iter()
                .take(MAX_COMPLETIONS)
                .map(|(_, argument)| AgentCompletion {
                    kind: AgentCompletionKind::Value,
                    label: argument.value.clone(),
                    detail: argument.detail.clone(),
                    description: argument.description.clone(),
                    replacement: format!("{} ", argument.value),
                    start: token.start,
                    end: token.end,
                })
                .collect();
        }
        if let Some(token) = slash_completion(text, cursor) {
            return self
                .command_candidates(token.query)
                .into_iter()
                .map(|candidate| AgentCompletion {
                    kind: AgentCompletionKind::Command,
                    label: format!("/{}", candidate.name),
                    detail: match candidate.source {
                        CommandSource::Mux => "Mux".to_owned(),
                        CommandSource::Agent => "ACP".to_owned(),
                    },
                    description: candidate.description,
                    replacement: format!("/{} ", candidate.name),
                    start: token.start,
                    end: token.end,
                })
                .collect();
        }
        if let Some(token) = mention_completion(text, cursor) {
            return self
                .file_candidates(token.query)
                .into_iter()
                .map(|entry| {
                    let display = entry.display;
                    AgentCompletion {
                        kind: AgentCompletionKind::File,
                        label: display.clone(),
                        detail: "File".to_owned(),
                        description: format!("Attach {display} to this prompt"),
                        replacement: format!("{} ", file_mention_token(&display)),
                        start: token.start,
                        end: token.end,
                    }
                })
                .collect();
        }
        Vec::new()
    }

    pub(crate) fn reference_paths(&self, text: &str) -> Vec<PathBuf> {
        let catalog = self.files.borrow();
        let Some(root) = catalog.root.as_ref() else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        file_mentions(text)
            .into_iter()
            .filter(|mention| seen.insert(mention.clone()))
            .take(MAX_REFERENCED_FILES)
            .filter_map(|mention| {
                catalog
                    .index
                    .lookup
                    .get(&mention)
                    .map(|path| root.join(path))
            })
            .collect()
    }

    fn command_candidates(&self, query: &str) -> Vec<CommandCandidate> {
        let mut seen = HashSet::new();
        let mut candidates = LOCAL_COMMANDS
            .iter()
            .map(|(name, description)| CommandCandidate {
                name: (*name).to_owned(),
                description: (*description).to_owned(),
                source: CommandSource::Mux,
            })
            .collect::<Vec<_>>();
        seen.extend(candidates.iter().map(|command| command.name.to_lowercase()));

        let agent_commands = self.agent_commands.borrow();
        candidates.extend(agent_commands.iter().filter_map(|command| {
            let key = command.name.to_lowercase();
            seen.insert(key).then(|| CommandCandidate {
                name: command.name.clone(),
                description: command.description.clone(),
                source: CommandSource::Agent,
            })
        }));

        let query = query.to_lowercase();
        if !query.is_empty() {
            candidates.sort_by_key(|candidate| command_rank(&candidate.name, &query));
        }
        candidates
            .into_iter()
            .filter(|candidate| command_rank(&candidate.name, &query).0 < u8::MAX)
            .take(MAX_COMPLETIONS)
            .collect()
    }

    fn file_candidates(&self, query: &str) -> Vec<IndexedFile> {
        let catalog = self.files.borrow();
        let query = query.to_lowercase();
        let mut paths = catalog
            .index
            .entries
            .iter()
            .filter_map(|entry| {
                let rank = file_rank(entry, &query);
                (rank.0 < u8::MAX).then_some((rank, entry))
            })
            .collect::<Vec<_>>();
        paths.sort_by(|(left_rank, left), (right_rank, right)| {
            left_rank
                .cmp(right_rank)
                .then_with(|| left.display.cmp(&right.display))
        });
        paths
            .into_iter()
            .map(|(_, entry)| entry.clone())
            .take(MAX_COMPLETIONS)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompletionToken<'a> {
    start: usize,
    end: usize,
    query: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ArgumentCompletionToken<'a> {
    command: &'a str,
    start: usize,
    end: usize,
    query: &'a str,
}

fn slash_completion(text: &str, cursor: usize) -> Option<CompletionToken<'_>> {
    token_completion(text, cursor, '/')
}

fn mention_completion(text: &str, cursor: usize) -> Option<CompletionToken<'_>> {
    token_completion(text, cursor, '@')
}

fn slash_argument_completion(text: &str, cursor: usize) -> Option<ArgumentCompletionToken<'_>> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let line = &text[line_start..cursor];
    let slash = line.find('/')?;
    if !line[..slash].chars().all(char::is_whitespace) {
        return None;
    }
    let command_start = slash + 1;
    let command_end = line[command_start..]
        .find(char::is_whitespace)
        .map(|offset| command_start + offset)?;
    let command = &line[command_start..command_end];
    if command.is_empty() {
        return None;
    }
    let argument_start = command_end
        + line[command_end..]
            .find(|character: char| !character.is_whitespace())
            .unwrap_or(line.len() - command_end);
    let query = &line[argument_start..];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    Some(ArgumentCompletionToken {
        command,
        start: line_start + argument_start,
        end: cursor,
        query,
    })
}

fn token_completion(text: &str, cursor: usize, trigger: char) -> Option<CompletionToken<'_>> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let line_start = text[..cursor].rfind('\n').map_or(0, |index| index + 1);
    let line = &text[line_start..cursor];
    for (index, _) in line.rmatch_indices(trigger) {
        let start = line_start + index;
        let query = &text[start + trigger.len_utf8()..cursor];
        if query.chars().any(char::is_whitespace) {
            continue;
        }
        if start > 0
            && text[..start].chars().next_back().is_some_and(|character| {
                !character.is_whitespace()
                    && (trigger != '@' || !matches!(character, '(' | '[' | '{'))
            })
        {
            continue;
        }
        return Some(CompletionToken {
            start,
            end: cursor,
            query,
        });
    }
    None
}

pub(crate) fn index_files(root: &Path) -> AgentFileIndex {
    let mut files = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|kind| kind.is_dir())
                || !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "target" | ".zig-cache" | "zig-out")
                )
        })
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .filter_map(|entry| entry.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .take(MAX_INDEXED_FILES)
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
    let entries = files
        .into_iter()
        .map(|path| {
            let display = normalized_relative_path(&path);
            let search = display.to_lowercase();
            let file_name = Path::new(&search)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&search)
                .to_owned();
            let depth = path.components().count();
            IndexedFile {
                path,
                display,
                search,
                file_name,
                depth,
            }
        })
        .collect::<Vec<_>>();
    let lookup = entries
        .iter()
        .map(|entry| (entry.display.clone(), entry.path.clone()))
        .collect();
    AgentFileIndex { entries, lookup }
}

fn file_mentions(text: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'@'
            || (index > 0
                && text[..index].chars().next_back().is_some_and(|character| {
                    !character.is_whitespace() && !matches!(character, '(' | '[' | '{')
                }))
        {
            index += 1;
            continue;
        }
        let start = index + 1;
        if bytes.get(start) == Some(&b'[')
            && let Some(end) = text[start + 1..].find(']')
        {
            mentions.push(text[start + 1..start + 1 + end].to_owned());
            index = start + end + 2;
            continue;
        }
        let end = text[start..]
            .find(char::is_whitespace)
            .map_or(text.len(), |offset| start + offset);
        if end > start {
            mentions.push(text[start..end].to_owned());
        }
        index = end.max(index + 1);
    }
    mentions
}

fn file_mention_token(path: &str) -> String {
    if path.chars().any(char::is_whitespace) {
        format!("@[{path}]")
    } else {
        format!("@{path}")
    }
}

fn normalized_relative_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn command_rank(name: &str, query: &str) -> (u8, usize, String) {
    let name = name.to_lowercase();
    if query.is_empty() || name == query {
        return (0, name.len(), name);
    }
    if name.starts_with(query) {
        return (1, name.len(), name);
    }
    if let Some(position) = name.find(query) {
        return (2, position, name);
    }
    if fuzzy_subsequence(&name, query) {
        return (3, name.len(), name);
    }
    (u8::MAX, usize::MAX, name)
}

fn file_rank(entry: &IndexedFile, query: &str) -> (u8, usize, usize, usize) {
    let path = entry.search.as_str();
    let file_name = entry.file_name.as_str();
    if query.is_empty() {
        return (0, entry.depth, path.len(), 0);
    }
    if file_name == query {
        return (0, 0, path.len(), 0);
    }
    if file_name.starts_with(query) {
        return (1, 0, path.len(), 0);
    }
    if path.starts_with(query) {
        return (2, entry.depth, path.len(), 0);
    }
    if let Some(position) = path.find(query) {
        return (3, position, path.len(), 0);
    }
    if fuzzy_subsequence(path, query) {
        return (4, entry.depth, path.len(), 0);
    }
    (u8::MAX, usize::MAX, usize::MAX, usize::MAX)
}

fn fuzzy_subsequence(value: &str, query: &str) -> bool {
    let mut query = query.chars();
    let mut next = query.next();
    for character in value.chars() {
        if next == Some(character) {
            next = query.next();
            if next.is_none() {
                return true;
            }
        }
    }
    next.is_none()
}

#[cfg(test)]
mod tests {
    use mux_acp::AgentSlashCommand;

    use super::{
        AgentCommandArgument, AgentCompletionMenu, AgentCompletionProvider, file_mention_token,
        file_mentions, mention_completion, slash_argument_completion, slash_completion,
    };

    #[test]
    fn slash_completion_requires_a_token_boundary_and_no_argument() {
        assert_eq!(
            slash_completion("/he", 3).map(|item| item.query),
            Some("he")
        );
        assert_eq!(
            slash_completion("ask /mod", 8).map(|item| item.query),
            Some("mod")
        );
        assert!(slash_completion("https://zed.dev", 15).is_none());
        assert!(slash_completion("/new codex", 10).is_none());
    }

    #[test]
    fn local_commands_win_over_agent_duplicates() {
        let provider = AgentCompletionProvider::default();
        provider.set_agent_commands(vec![AgentSlashCommand {
            name: "help".to_owned(),
            description: "Agent help".to_owned(),
        }]);

        let matches = provider.command_candidates("help");
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].description,
            "Show keyboard controls and available commands in the conversation"
        );
    }

    #[test]
    fn command_matching_prefers_exact_then_prefix() {
        let provider = AgentCompletionProvider::default();
        let matches = provider.command_candidates("ne");
        assert_eq!(matches[0].name, "new");
        assert!(matches.iter().any(|candidate| candidate.name == "next"));
    }

    #[test]
    fn slash_arguments_complete_from_the_live_catalog() {
        let provider = AgentCompletionProvider::default();
        provider.set_command_arguments(vec![
            AgentCommandArgument {
                command: "new".to_owned(),
                value: "codex-acp".to_owned(),
                detail: "Agent".to_owned(),
                description: "Codex over ACP".to_owned(),
            },
            AgentCommandArgument {
                command: "new".to_owned(),
                value: "gemini".to_owned(),
                detail: "Agent".to_owned(),
                description: "Gemini over ACP".to_owned(),
            },
        ]);

        let matches = provider.completions("/new cod", 8);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].replacement, "codex-acp ");
        assert_eq!(matches[0].start, 5);
        assert_eq!(provider.completions("/new ", 5)[0].label, "codex-acp");
        assert_eq!(
            slash_argument_completion(" /new ", 6).map(|token| token.command),
            Some("new")
        );
        assert!(slash_argument_completion("/new codex /tmp", 15).is_none());
    }

    #[test]
    fn mentions_complete_only_at_text_boundaries() {
        assert_eq!(
            mention_completion("review @src/ma", 14).map(|item| item.query),
            Some("src/ma")
        );
        assert!(mention_completion("name@example.com", 16).is_none());
        assert_eq!(
            mention_completion("(@README", 8).map(|item| item.query),
            Some("README")
        );
    }

    #[test]
    fn file_mentions_preserve_paths_with_spaces() {
        assert_eq!(
            file_mention_token("docs/agent ux.md"),
            "@[docs/agent ux.md]"
        );
        assert_eq!(
            file_mentions("compare @src/main.rs with @[docs/agent ux.md]"),
            ["src/main.rs", "docs/agent ux.md"]
        );
    }

    #[test]
    fn menu_navigation_wraps_without_losing_selection() {
        let provider = AgentCompletionProvider::default();
        let mut menu = AgentCompletionMenu {
            items: provider.completions("/", 1),
            selected: 0,
        };
        menu.select_relative(-1);
        assert_eq!(menu.selected, menu.items.len() - 1);
        menu.select_relative(1);
        assert_eq!(menu.selected, 0);
    }
}
