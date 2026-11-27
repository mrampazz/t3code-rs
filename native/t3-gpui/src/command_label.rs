//! Command labels as the web reads them (`client-runtime/src/work-log/commandLabel.ts`): the
//! script a shell wrapper runs (`commandDisplayText`) and the program a command line runs
//! (`commandProgramName`), found without executing anything.

use std::sync::LazyLock;

use regex::Regex;

const MAX_COMMAND_SEGMENTS: usize = 64;

const SHELL_PROGRAMS: [&str; 7] = ["sh", "bash", "zsh", "dash", "ash", "ksh", "fish"];
const WINDOWS_SHELL_PROGRAMS: [&str; 6] = [
    "cmd",
    "cmd.exe",
    "powershell",
    "powershell.exe",
    "pwsh",
    "pwsh.exe",
];
const SHELL_OPTIONS_WITH_VALUE: [&str; 4] = ["-o", "-O", "--rcfile", "--init-file"];
const SHELL_COMMAND_WRAPPERS: [&str; 3] = ["builtin", "command", "exec"];
const SHELL_PRECOMMAND_MODIFIERS: [&str; 3] = ["nocorrect", "noglob", "time"];
const POWERSHELL_SETUP_PROGRAMS: [&str; 3] = ["pop-location", "push-location", "set-location"];
const POWERSHELL_FLAGS: [&str; 5] = ["-mta", "-nologo", "-noninteractive", "-noprofile", "-sta"];
const POWERSHELL_OPTIONS_WITH_VALUE: [&str; 7] = [
    "-configurationname",
    "-executionpolicy",
    "-inputformat",
    "-outputformat",
    "-version",
    "-windowstyle",
    "-workingdirectory",
];
const START_PROCESS_FLAGS: [&str; 9] = [
    "-confirm",
    "-debug",
    "-loaduserprofile",
    "-nonewwindow",
    "-passthru",
    "-usenewenvironment",
    "-verbose",
    "-wait",
    "-whatif",
];
const START_PROCESS_OPTIONS_WITH_VALUE: [&str; 19] = [
    "-argumentlist",
    "-credential",
    "-environment",
    "-erroraction",
    "-errorvariable",
    "-informationaction",
    "-informationvariable",
    "-outbuffer",
    "-outvariable",
    "-pipelinevariable",
    "-progressaction",
    "-redirectstandarderror",
    "-redirectstandardinput",
    "-redirectstandardoutput",
    "-verb",
    "-warningaction",
    "-warningvariable",
    "-windowstyle",
    "-workingdirectory",
];
const SKIPPABLE_SUDO_PROBES: [&str; 4] = ["[", "[[", "test", "true"];
const NON_PROGRAM_PREFIX_CHARACTERS: &str = "<>(){}[];|&$`#!%@:";
const NON_PROGRAM_SUFFIX_CHARACTERS: &str = "){]}`";

const NON_DESCRIPTIVE_SHELL_PROGRAMS: [&str; 92] = [
    "!",
    "#",
    ".",
    ":",
    "[",
    "[[",
    "alias",
    "and",
    "autoload",
    "begin",
    "bg",
    "bind",
    "bindkey",
    "break",
    "builtin",
    "caller",
    "case",
    "catch",
    "cd",
    "command",
    "compgen",
    "complete",
    "compopt",
    "continue",
    "coproc",
    "declare",
    "dirs",
    "disown",
    "do",
    "done",
    "elif",
    "else",
    "enable",
    "end",
    "esac",
    "eval",
    "exec",
    "exit",
    "export",
    "false",
    "fc",
    "fg",
    "fi",
    "finally",
    "for",
    "foreach",
    "function",
    "getopts",
    "history",
    "if",
    "in",
    "jobs",
    "let",
    "local",
    "logout",
    "mapfile",
    "nocorrect",
    "noglob",
    "not",
    "or",
    "popd",
    "pushd",
    "read",
    "readarray",
    "readonly",
    "repeat",
    "return",
    "select",
    "set",
    "setopt",
    "shift",
    "shopt",
    "source",
    "switch",
    "suspend",
    "test",
    "then",
    "time",
    "times",
    "trap",
    "try",
    "true",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "unalias",
    "until",
    "unset",
    "unsetopt",
    "wait",
    "while",
];

const TERMINAL_SHELL_PROGRAMS: [&str; 33] = [
    "and", "begin", "break", "case", "catch", "continue", "coproc", "do", "done", "elif", "else",
    "end", "esac", "exec", "exit", "false", "fi", "finally", "for", "foreach", "function", "if",
    "in", "not", "or", "repeat", "return", "select", "switch", "then", "try", "until", "while",
];

#[derive(Clone, Copy, PartialEq)]
enum Wrapper {
    Env,
    Sudo,
}

fn wrapper_options_with_value(wrapper: Wrapper) -> &'static [&'static str] {
    match wrapper {
        Wrapper::Env => &["-C", "--chdir", "-S", "--split-string", "-u", "--unset"],
        Wrapper::Sudo => &[
            "-C",
            "--close-from",
            "-D",
            "--chdir",
            "-g",
            "--group",
            "-u",
            "--user",
        ],
    }
}

fn wrapper_flags(wrapper: Wrapper) -> &'static [&'static str] {
    match wrapper {
        Wrapper::Env => &[
            "-0",
            "--null",
            "-i",
            "--ignore-environment",
            "--debug",
            "-v",
        ],
        Wrapper::Sudo => &[
            "-A",
            "--askpass",
            "-b",
            "--background",
            "-E",
            "-H",
            "-i",
            "-n",
            "-S",
        ],
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Context {
    Exec,
    Shell,
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid command label regex")
}

static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*\+?="));
static REDIRECTION: LazyLock<Regex> = LazyLock::new(|| {
    re(
        r"(?s)^(?:(?:(?:\d+|\*|\{[A-Za-z_][A-Za-z0-9_]*\})?(?:<<<|<<-|<<|<>|>>|>\||<&|>&|<|>))|&>>|&>)(.*)$",
    )
});

/// `shellCommandArgumentIndex`: the token after `-c` / `-lc` / `--command`.
fn shell_command_argument_index(tokens: &[String], start: usize) -> Option<usize> {
    let mut index = start;
    while index < tokens.len() {
        let option = tokens[index].as_str();
        if option == "--" || !option.starts_with('-') {
            return None;
        }
        if SHELL_OPTIONS_WITH_VALUE.contains(&option) {
            index += 2;
            continue;
        }
        let letters = &option[1..];
        if option == "--command"
            || (letters.contains('c') && letters.chars().all(|c| c.is_ascii_alphabetic()))
        {
            return Some(index + 1);
        }
        index += 1;
    }
    None
}

fn is_windows_path_prefix(current: &str) -> bool {
    static WINDOWS: LazyLock<Regex> = LazyLock::new(|| re(r"^(?:[A-Za-z]:|\.{1,2})(?:\\[^\s]*)?$"));
    WINDOWS.is_match(current)
}

/// `tokenizeShellCommand`: words with quotes removed; `None` when quoting never closes.
pub(crate) fn tokenize(command: &str) -> Option<Vec<String>> {
    let chars: Vec<char> = command.trim().chars().collect();
    let mut tokens = Vec::new();
    let mut current = String::new();
    let (mut quote, mut escaping, mut backticks) = (None::<char>, false, false);
    let (mut substitution, mut expansion, mut started) = (0usize, 0usize, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        i += 1;
        if escaping {
            current.push(c);
            escaping = false;
            started = true;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            let windows_path = quote.is_none() && is_windows_path_prefix(&current);
            if (quote == Some('"') || windows_path)
                && next.is_some_and(|n| !['"', '\\', '$', '`', '\n'].contains(&n))
            {
                current.push(c);
                started = true;
                continue;
            }
            escaping = true;
            started = true;
            continue;
        }
        if backticks {
            current.push(c);
            backticks = c != '`';
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
            started = true;
            continue;
        }
        match c {
            '`' => {
                current.push(c);
                backticks = true;
            }
            '$' if next == Some('{') => {
                current.push_str("${");
                expansion += 1;
                i += 1;
            }
            '{' if expansion > 0 => {
                current.push(c);
                expansion += 1;
            }
            '}' if expansion > 0 => {
                current.push(c);
                expansion -= 1;
            }
            '$' if next == Some('(') => {
                current.push_str("$(");
                substitution += 1;
                i += 1;
            }
            '(' => {
                current.push(c);
                substitution += 1;
            }
            ')' if substitution > 0 => {
                current.push(c);
                substitution -= 1;
            }
            '"' | '\'' => quote = Some(c),
            c if c.is_whitespace() => {
                if substitution > 0 || expansion > 0 {
                    current.push(c);
                } else {
                    if started {
                        tokens.push(std::mem::take(&mut current));
                        started = false;
                    }
                    continue;
                }
            }
            c => current.push(c),
        }
        started = true;
    }
    if quote.is_some() || escaping || backticks || substitution > 0 || expansion > 0 {
        return None;
    }
    if started {
        tokens.push(current);
    }
    Some(tokens)
}

/// `ShellCommandSplit`.
pub(crate) struct Split {
    pub first: String,
    pub remaining: Option<String>,
    pub separator: Option<String>,
}

struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
}

fn without_comments(chars: &[char], end: usize, comments: &[(usize, usize)]) -> String {
    let mut out = String::new();
    let mut cursor = 0;
    for &(start, stop) in comments {
        if start >= end {
            break;
        }
        out.extend(&chars[cursor..start]);
        cursor = stop.min(end);
    }
    out.extend(&chars[cursor.min(end)..end]);
    out
}

fn read_heredoc_delimiter(
    chars: &[char],
    start: usize,
    strip_tabs: bool,
) -> Option<(Heredoc, usize)> {
    let mut index = start;
    while matches!(chars.get(index), Some(' ' | '\t')) {
        index += 1;
    }
    let mut delimiter = String::new();
    let (mut quote, mut escaping) = (None::<char>, false);
    while index < chars.len() {
        let c = chars[index];
        if escaping {
            delimiter.push(c);
            escaping = false;
        } else if c == '\\' && quote != Some('\'') {
            escaping = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                delimiter.push(c);
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c.is_whitespace() || ";&|<>()".contains(c) {
            break;
        } else {
            delimiter.push(c);
        }
        index += 1;
    }
    if delimiter.is_empty() || quote.is_some() || escaping {
        return None;
    }
    Some((
        Heredoc {
            delimiter,
            strip_tabs,
        },
        index,
    ))
}

fn after_heredocs(chars: &[char], start: usize, heredocs: &[Heredoc]) -> Option<String> {
    let text: String = chars.iter().collect();
    // Work in byte offsets of `text` from here on.
    let mut cursor = chars[..start.min(chars.len())]
        .iter()
        .map(|c| c.len_utf8())
        .sum::<usize>();
    for heredoc in heredocs {
        let mut found = false;
        while cursor <= text.len() {
            let newline = text[cursor..].find('\n').map(|n| n + cursor);
            let line_end = newline.unwrap_or(text.len());
            let line = text[cursor..line_end].trim_end_matches('\r');
            let comparable = if heredoc.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            cursor = newline.map_or(text.len(), |n| n + 1);
            if comparable == heredoc.delimiter {
                found = true;
                break;
            }
            if newline.is_none() {
                break;
            }
        }
        if !found {
            return None;
        }
    }
    let rest = text[cursor.min(text.len())..].trim();
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// `splitFirstShellCommand`: the first command, what follows its top-level separator (heredoc
/// bodies skipped), and the separator.
pub(crate) fn split_first(command: &str) -> Split {
    let chars: Vec<char> = command.chars().collect();
    let at = |k: usize| chars.get(k).copied();
    let (mut quote, mut here_string) = (None::<char>, None::<char>);
    let (mut escaping, mut backticks, mut in_comment) = (false, false, false);
    let (mut substitution, mut expansion) = (0usize, 0usize);
    let mut heredocs: Vec<Heredoc> = Vec::new();
    let mut comments: Vec<(usize, usize)> = Vec::new();
    let mut comment_start = 0;
    let mut separator_before_heredocs: Option<(usize, usize)> = None;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        if let Some(q) = here_string {
            if c == q && at(index + 1) == Some('@') && (index == 0 || chars[index - 1] == '\n') {
                here_string = None;
                index += 1;
            }
            index += 1;
            continue;
        }
        if in_comment {
            if c != '\n' {
                index += 1;
                continue;
            }
            in_comment = false;
            comments.push((comment_start, index));
        }
        if escaping {
            escaping = false;
            index += 1;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escaping = true;
            index += 1;
            continue;
        }
        if backticks {
            backticks = c != '`';
            index += 1;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            index += 1;
            continue;
        }
        if c == '@'
            && matches!(at(index + 1), Some('"' | '\''))
            && (at(index + 2) == Some('\n')
                || (at(index + 2) == Some('\r') && at(index + 3) == Some('\n')))
        {
            here_string = at(index + 1);
            index += 2;
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            index += 1;
            continue;
        }
        if c == '`' {
            backticks = true;
            index += 1;
            continue;
        }
        if c == '#'
            && (index == 0 || chars[index - 1].is_whitespace() || ";&|(".contains(chars[index - 1]))
        {
            in_comment = true;
            comment_start = index;
            index += 1;
            continue;
        }
        if c == '$' && at(index + 1) == Some('{') {
            expansion += 1;
            index += 2;
            continue;
        }
        if c == '{' && expansion > 0 {
            expansion += 1;
            index += 1;
            continue;
        }
        if c == '}' && expansion > 0 {
            expansion -= 1;
            index += 1;
            continue;
        }
        if c == '(' {
            substitution += 1;
            index += 1;
            continue;
        }
        if c == ')' && substitution > 0 {
            substitution -= 1;
            index += 1;
            continue;
        }
        if substitution > 0 || expansion > 0 {
            index += 1;
            continue;
        }
        if c == '<' && at(index + 1) == Some('<') && at(index + 2) != Some('<') {
            let strip_tabs = at(index + 2) == Some('-');
            match read_heredoc_delimiter(&chars, index + if strip_tabs { 3 } else { 2 }, strip_tabs)
            {
                None => {
                    return Split {
                        first: command.trim().to_owned(),
                        remaining: None,
                        separator: None,
                    };
                }
                Some((heredoc, end)) => {
                    heredocs.push(heredoc);
                    index = end;
                    continue;
                }
            }
        }
        let double = (c == '&' && at(index + 1) == Some('&'))
            || (c == '|' && matches!(at(index + 1), Some('|' | '&')));
        let redirection_ampersand = c == '&'
            && ((index > 0 && matches!(chars[index - 1], '>' | '<')) || at(index + 1) == Some('>'));
        if (!double && !";&|\n".contains(c)) || redirection_ampersand {
            index += 1;
            continue;
        }
        if c == '\n' && !heredocs.is_empty() {
            let separator = separator_before_heredocs;
            let first = without_comments(&chars, separator.map_or(index, |s| s.0), &comments)
                .trim_start()
                .to_owned();
            let before: String = separator
                .map(|(at, len)| chars[at + len..index].iter().collect::<String>())
                .unwrap_or_default();
            let following = after_heredocs(&chars, index + 1, &heredocs);
            let remaining = [Some(before.trim().to_owned()), following]
                .into_iter()
                .flatten()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            return Split {
                first,
                remaining: (!remaining.is_empty()).then_some(remaining),
                separator: Some(match separator {
                    Some((at, len)) => chars[at..at + len].iter().collect(),
                    None => "\n".into(),
                }),
            };
        }
        if !heredocs.is_empty() {
            if separator_before_heredocs.is_none() {
                separator_before_heredocs = Some((index, if double { 2 } else { 1 }));
            }
            index += if double { 2 } else { 1 };
            continue;
        }
        let first = without_comments(&chars, index, &comments)
            .trim_start()
            .to_owned();
        let mut next = index + if double { 2 } else { 1 };
        while chars.get(next).is_some_and(|c| c.is_whitespace()) {
            next += 1;
        }
        let rest: String = chars[next.min(chars.len())..].iter().collect();
        let rest = rest.trim();
        return Split {
            first,
            remaining: (!rest.is_empty()).then(|| rest.to_owned()),
            separator: Some(
                chars[index..index + if double { 2 } else { 1 }]
                    .iter()
                    .collect(),
            ),
        };
    }
    if in_comment {
        comments.push((comment_start, chars.len()));
    }
    Split {
        first: without_comments(&chars, chars.len(), &comments)
            .trim()
            .to_owned(),
        remaining: None,
        separator: None,
    }
}

/// `commandWithoutLeadingShellComments`.
fn without_leading_comments(command: &str) -> Option<String> {
    let mut rest = command.trim_start();
    while rest.starts_with('#') {
        rest = rest[rest.find('\n')? + 1..].trim_start();
    }
    (!rest.is_empty()).then(|| rest.to_owned())
}

/// `withoutShellLineContinuations`.
fn without_line_continuations(command: &str) -> String {
    let chars: Vec<char> = command.chars().collect();
    let mut out = String::new();
    let (mut quote, mut escaping) = (None::<char>, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if escaping {
            out.push(c);
            escaping = false;
            i += 1;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            if chars.get(i + 1) == Some(&'\n') {
                i += 2;
                continue;
            }
            if chars.get(i + 1) == Some(&'\r') && chars.get(i + 2) == Some(&'\n') {
                i += 3;
                continue;
            }
            out.push(c);
            escaping = true;
            i += 1;
            continue;
        }
        match quote {
            Some(q) if c == q => quote = None,
            None if c == '"' || c == '\'' => quote = Some(c),
            _ => {}
        }
        out.push(c);
        i += 1;
    }
    out
}

/// `indexAfterShellRedirection`: past a redirection and its target, if `tokens[index]` is one.
fn index_after_redirection(tokens: &[String], index: usize) -> Option<usize> {
    let token = tokens.get(index)?;
    if token.starts_with("<(") || token.starts_with(">(") {
        return None;
    }
    let captures = REDIRECTION.captures(token)?;
    if !captures[1].is_empty() {
        return Some(index + 1);
    }
    Some(if tokens.get(index + 1).is_none() {
        tokens.len() + 1
    } else {
        index + 2
    })
}

fn serialize_tokens(tokens: &[String]) -> String {
    tokens
        .iter()
        .map(|t| format!("'{}'", t.replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `transparentWrapperCommandIndex`: `bundle exec`, `nohup`, BSD `script`, `arch`, `timeout`.
fn transparent_wrapper_index(wrapper: &str, tokens: &[String], index: usize) -> Option<usize> {
    let token = |k: usize| tokens.get(k).map(String::as_str);
    match wrapper {
        "bundle" => {
            (token(index + 1) == Some("exec") && token(index + 2).is_some()).then_some(index + 2)
        }
        "nohup" => {
            let target = if token(index + 1) == Some("--") {
                index + 2
            } else {
                index + 1
            };
            token(target)
                .filter(|t| !t.starts_with('-'))
                .map(|_| target)
        }
        "script" => {
            static OPTIONS: LazyLock<Regex> = LazyLock::new(|| re(r"^-[adkpqr]+$"));
            (OPTIONS.is_match(token(index + 1).unwrap_or("")) && token(index + 3).is_some())
                .then_some(index + 3)
        }
        "arch" => {
            static ARCH: LazyLock<Regex> = LazyLock::new(|| re(r"^-(?:arm64|arm64e|i386|x86_64)$"));
            if ARCH.is_match(token(index + 1).unwrap_or("")) {
                return token(index + 2).is_some().then_some(index + 2);
            }
            (token(index + 1) == Some("-arch") && token(index + 3).is_some()).then_some(index + 3)
        }
        "timeout" | "gtimeout" => {
            static DURATION: LazyLock<Regex> =
                LazyLock::new(|| re(r"^(?:\d+(?:\.\d*)?|\.\d+)[smhd]?$"));
            (DURATION.is_match(token(index + 1).unwrap_or("")) && token(index + 2).is_some())
                .then_some(index + 2)
        }
        _ => None,
    }
}

/// `staticProgramName`: a path's last component, when it reads as a program name.
fn static_program_name(value: &str) -> Option<String> {
    static URL_SCHEME: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z][A-Za-z0-9+.-]*:[^\\/]"));
    let trimmed = value.trim();
    if trimmed.is_empty() || URL_SCHEME.is_match(trimmed) || {
        // `scheme:` at the end also counts as a URL-like value.
        let colon = trimmed.find(':');
        colon.is_some_and(|c| {
            c + 1 == trimmed.len()
                && trimmed[..c].chars().enumerate().all(|(i, ch)| {
                    if i == 0 {
                        ch.is_ascii_alphabetic()
                    } else {
                        ch.is_ascii_alphanumeric() || "+.-".contains(ch)
                    }
                })
        })
    } {
        return None;
    }
    let program = trimmed.rsplit(['/', '\\']).next()?;
    let has_separator = trimmed.contains(['/', '\\']);
    if program.is_empty()
        || (program.contains(char::is_whitespace) && !has_separator)
        || program
            .chars()
            .next()
            .is_some_and(|c| NON_PROGRAM_PREFIX_CHARACTERS.contains(c))
        || program
            .chars()
            .last()
            .is_some_and(|c| NON_PROGRAM_SUFFIX_CHARACTERS.contains(c))
    {
        return None;
    }
    Some(program.to_owned())
}

/// `leadingPowerShellLiteral`.
fn leading_powershell_literal(command: &str) -> Option<String> {
    let input = command.trim_start();
    let mut chars = input.chars();
    let quote = chars.next()?;
    if quote != '"' && quote != '\'' {
        return static_program_name(input.split_whitespace().next().unwrap_or(""));
    }
    let rest: Vec<char> = chars.collect();
    let mut value = String::new();
    let mut i = 0;
    while i < rest.len() {
        let c = rest[i];
        if c == '`' && i + 1 < rest.len() {
            value.push(rest[i + 1]);
            i += 2;
            continue;
        }
        if c == quote {
            if quote == '\'' && rest.get(i + 1) == Some(&'\'') {
                value.push('\'');
                i += 2;
                continue;
            }
            return static_program_name(&value);
        }
        value.push(c);
        i += 1;
    }
    None
}

/// `powerShellCallOperatorProgramName`: `& 'C:\tool.exe' …`. `Some(None)` when it is a call
/// with no readable program.
fn powershell_call_program(command: &str) -> Option<Option<String>> {
    static CALL: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)^\s*&\s+(.*)$"));
    let captures = CALL.captures(command)?;
    Some(leading_powershell_literal(&captures[1]))
}

/// `powerShellAssignmentProgramName`: `Some(program)` when the command is an assignment.
fn powershell_assignment_program(
    command: &str,
    depth: usize,
    remaining: Option<&str>,
    segments: usize,
) -> Option<Option<String>> {
    static ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
        re(r"(?is)^\s*\$(?:(env|global|local|script):)?[A-Za-z_][A-Za-z0-9_]*\s*=\s*(.*)$")
    });
    static HASH: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^(?:\[ordered\]\s*)?@\s*[{(]"));
    static DIRECT: LazyLock<Regex> =
        LazyLock::new(|| re(r"^(?:@?\(\s*)?([A-Za-z][A-Za-z0-9_.-]*)\b"));
    let captures = ASSIGN.captures(command)?;
    let rest = |segments: usize| {
        remaining.and_then(|r| program_name_internal(r, depth, Context::Shell, segments - 1))
    };
    if captures
        .get(1)
        .is_some_and(|scope| scope.as_str().eq_ignore_ascii_case("env"))
    {
        return Some(rest(segments));
    }
    let value = captures[2].trim();
    if HASH.is_match(value) {
        return Some(None);
    }
    if let Some(called) = powershell_call_program(value) {
        return Some(called);
    }
    if let Some(direct) = DIRECT.captures(value).map(|c| c[1].to_owned())
        && !NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&direct.to_lowercase().as_str())
    {
        let parsed = program_name_internal(value, depth + 1, Context::Shell, segments);
        return Some(parsed.or(Some(direct)));
    }
    Some(rest(segments))
}

/// `windowsShellPayloadProgramName`: `Some(program)` when the shell's payload was found.
fn windows_payload_program(
    shell: &str,
    tokens: &[String],
    start: usize,
    depth: usize,
    remaining: Option<&str>,
    separator: Option<&str>,
    segments: usize,
) -> Option<Option<String>> {
    let parse = |payload: Option<&String>| -> Option<String> {
        let payload = payload?;
        let command = match (remaining, separator) {
            (Some(rest), Some(sep)) => format!("{payload} {sep} {rest}"),
            _ => payload.clone(),
        };
        program_name_internal(&command, depth + 1, Context::Shell, segments)
    };
    if shell == "cmd" || shell == "cmd.exe" {
        for index in start..tokens.len() {
            let option = tokens[index].to_lowercase();
            if option == "/c" || option == "/k" {
                return Some(parse(tokens.get(index + 1)));
            }
        }
        return None;
    }
    let mut index = start;
    while index < tokens.len() {
        let option = tokens[index].to_lowercase();
        match option.as_str() {
            "-command" | "-c" => return Some(parse(tokens.get(index + 1))),
            "-file" | "-f" => {
                return Some(static_program_name(
                    tokens.get(index + 1).map_or("", String::as_str),
                ));
            }
            "-encodedcommand" | "-enc" | "-e" => return Some(None),
            o if POWERSHELL_OPTIONS_WITH_VALUE.contains(&o) => index += 1,
            o if POWERSHELL_FLAGS.contains(&o) => {}
            o if !o.starts_with('-') => return Some(static_program_name(&tokens[index])),
            _ => {}
        }
        index += 1;
    }
    None
}

/// `startProcessProgramName`.
fn start_process_program(tokens: &[String], start: usize) -> Option<String> {
    let mut index = start;
    while index < tokens.len() {
        let token = &tokens[index];
        let option = token.to_lowercase();
        if option == "-filepath" {
            return static_program_name(tokens.get(index + 1).map_or("", String::as_str));
        }
        if START_PROCESS_FLAGS.contains(&option.as_str()) {
            index += 1;
            continue;
        }
        if START_PROCESS_OPTIONS_WITH_VALUE.contains(&option.as_str()) {
            tokens.get(index + 1)?;
            index += 2;
            continue;
        }
        if token.starts_with('-') {
            return None;
        }
        return static_program_name(token);
    }
    None
}

/// `literalAssignmentProgram`: `NAME=value` with the program the value names, if any.
fn literal_assignment_program(token: &str) -> Option<(String, Option<String>)> {
    static ASSIGN: LazyLock<Regex> = LazyLock::new(|| re(r"(?s)^([A-Za-z_][A-Za-z0-9_]*)=(.*)$"));
    let captures = ASSIGN.captures(token)?;
    let name = captures[1].to_owned();
    let mut value = captures[2].trim().to_owned();
    if value.is_empty() || value.contains(['$', '`']) {
        return Some((name, None));
    }
    if value.starts_with('(') && value.ends_with(')') && value.len() >= 2 {
        value = tokenize(&value[1..value.len() - 1])
            .and_then(|t| t.into_iter().next())
            .unwrap_or_default();
    } else if value.contains(char::is_whitespace) && !value.contains(['/', '\\']) {
        return Some((name, None));
    }
    Some((name, static_program_name(&value)))
}

/// `referencedCommandAlias`: `$NAME`, `${NAME}`, `${NAME[@]}`.
fn referenced_alias(token: &str) -> Option<String> {
    static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
        re(r"^\$(?:([A-Za-z_][A-Za-z0-9_]*)|\{([A-Za-z_][A-Za-z0-9_]*)(?:\[@\])?\})$")
    });
    let captures = REFERENCE.captures(token)?;
    captures
        .get(1)
        .or(captures.get(2))
        .map(|m| m.as_str().to_owned())
}

/// `literalCommandAliasProgramName`: `PY=python3; $PY script.py` runs python3.
fn literal_alias_program(command: &str) -> Option<String> {
    let mut aliases: Vec<(String, String)> = Vec::new();
    let mut remaining = Some(command.to_owned());
    let mut control_depth = 0usize;
    let mut segments = 0;
    while let Some(rest) = remaining.filter(|_| segments < 64) {
        segments += 1;
        let rest = without_leading_comments(&rest)?;
        let split = split_first(&rest);
        let tokens = tokenize(&without_line_continuations(&split.first))?;
        let mut index = usize::from(matches!(
            tokens.first().map(String::as_str),
            Some("do" | "then")
        ));
        while index < tokens.len() {
            if let Some(after) = index_after_redirection(&tokens, index)
                && after <= tokens.len()
            {
                index = after;
                continue;
            }
            if ASSIGNMENT.is_match(&tokens[index]) {
                index += 1;
                continue;
            }
            break;
        }
        if let Some(name) = tokens.get(index).and_then(|t| referenced_alias(t))
            && let Some((_, program)) = aliases.iter().find(|(n, _)| *n == name)
        {
            return Some(program.clone());
        }
        let leading = tokens.first().map(String::as_str);
        if matches!(leading, Some("fi" | "done" | "esac")) {
            control_depth = control_depth.saturating_sub(1);
        }
        if matches!(
            leading,
            Some("if" | "for" | "while" | "until" | "select" | "case")
        ) {
            control_depth += 1;
        }
        if control_depth == 0 && leading == Some("unset") {
            aliases.retain(|(n, _)| !tokens[1..].contains(n));
        }
        let start = usize::from(leading == Some("export"));
        let assignments: Vec<Option<(String, Option<String>)>> = tokens[start.min(tokens.len())..]
            .iter()
            .map(|t| literal_assignment_program(t))
            .collect();
        if control_depth == 0 && !assignments.is_empty() && assignments.iter().all(Option::is_some)
        {
            for (name, program) in assignments.into_iter().flatten() {
                aliases.retain(|(n, _)| *n != name);
                if let Some(program) = program {
                    aliases.push((name, program));
                }
            }
        }
        remaining = split.remaining;
    }
    None
}

/// `wrappedShellCommandProgramName`: what `command`, `builtin` or `exec` runs.
fn wrapped_shell_program(
    wrapper: &str,
    tokens: &[String],
    start: usize,
    depth: usize,
    remaining: Option<&str>,
    segments: usize,
) -> Option<String> {
    let mut index = start;
    match wrapper {
        "command" => {
            while index < tokens.len() {
                let option = tokens[index].as_str();
                if option == "--" {
                    index += 1;
                    break;
                }
                if !option.starts_with('-') || option == "-" {
                    break;
                }
                if option != "-p" {
                    return None;
                }
                index += 1;
            }
        }
        "builtin" => {
            if tokens.get(index).map(String::as_str) == Some("--") {
                index += 1;
            } else if tokens.get(index).is_some_and(|t| t.starts_with('-')) {
                return None;
            }
        }
        "exec" => {
            static EXEC_FLAGS: LazyLock<Regex> = LazyLock::new(|| re(r"^-[cl]+$"));
            while index < tokens.len() {
                let option = tokens[index].as_str();
                if option == "--" {
                    index += 1;
                    break;
                }
                if option == "-a" {
                    tokens.get(index + 1)?;
                    index += 2;
                    continue;
                }
                if (option.starts_with("-a") && option.len() > 2) || EXEC_FLAGS.is_match(option) {
                    index += 1;
                    continue;
                }
                if option.starts_with('-') && option != "-" {
                    return None;
                }
                break;
            }
        }
        _ => {}
    }
    let wrapped = &tokens[index.min(tokens.len())..];
    if wrapped.is_empty() {
        return None;
    }
    let mut target = 0;
    while target < wrapped.len() {
        match index_after_redirection(wrapped, target) {
            Some(after) if after <= wrapped.len() => target = after,
            _ => break,
        }
    }
    let target_token = wrapped.get(target);
    if target_token.is_some_and(|t| ASSIGNMENT.is_match(t)) {
        return None;
    }
    let context = if wrapper == "exec" {
        Context::Exec
    } else {
        Context::Shell
    };
    if let Some(program) =
        program_name_internal(&serialize_tokens(wrapped), depth + 1, context, segments)
    {
        return Some(program);
    }
    let target = target_token?.as_str();
    if wrapper != "exec"
        && target == target.to_lowercase()
        && NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&target)
        && !TERMINAL_SHELL_PROGRAMS.contains(&target)
        && let Some(rest) = remaining
    {
        return program_name_internal(rest, depth, Context::Shell, segments - 1);
    }
    None
}

/// `parseCommandProgramName`.
fn parse_program(command: &str, depth: usize, context: Context, segments: usize) -> Option<String> {
    static BLOCK: LazyLock<Regex> = LazyLock::new(|| {
        re(r"(?i)^(?:catch|finally|for|foreach|function|if|param|switch|try|while)\s*[{(]")
    });
    static FUNCTION: LazyLock<Regex> =
        LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*\s*\(\s*\)\s*\{"));
    static HERE_STRING: LazyLock<Regex> = LazyLock::new(|| re("^@[\"'](?:\r?\n)"));
    static WINDOWS_PATH: LazyLock<Regex> = LazyLock::new(|| {
        re(r"(?i)^\s*((?:\.{1,2}|%[A-Za-z_][A-Za-z0-9_]*%|\$env:[A-Za-z_][A-Za-z0-9_]*)\\\S+)")
    });
    static FUNCTION_TOKEN: LazyLock<Regex> =
        LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*\(\)\{$"));
    if depth >= 8 || segments == 0 {
        return None;
    }
    let command = without_leading_comments(command)?;
    if BLOCK.is_match(&command) || FUNCTION.is_match(&command) || command.starts_with("[[") {
        return None;
    }
    let split = split_first(&command);
    let remaining = split.remaining.as_deref();
    let next = |context: Context| {
        remaining.and_then(|r| program_name_internal(r, depth, context, segments - 1))
    };
    if HERE_STRING.is_match(split.first.trim_start()) {
        return next(Context::Shell);
    }
    if let Some(program) = powershell_assignment_program(&split.first, depth, remaining, segments) {
        return program;
    }
    if let Some(path) = WINDOWS_PATH.captures(&split.first) {
        return static_program_name(&path[1]);
    }
    let tokens = tokenize(&without_line_continuations(&split.first))?;
    let first_char = split.first.trim_start().chars().next();
    if tokens.len() == 1
        && matches!(first_char, Some('"' | '\''))
        && tokens[0].contains([' ', '\t', '\n', '(', ')', '='])
        && !tokens[0].contains(['/', '\\'])
    {
        return next(context);
    }
    let mut index = 0;
    let mut wrapper: Option<Wrapper> = None;
    let mut context = context;
    let (mut saw_assignment, mut saw_redirection) = (false, false);
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token.is_empty() {
            return None;
        }
        if let Some(after) = index_after_redirection(&tokens, index) {
            if after > tokens.len() {
                return None;
            }
            saw_redirection = true;
            index = after;
            continue;
        }
        if ASSIGNMENT.is_match(token) {
            saw_assignment = true;
            index += 1;
            continue;
        }
        if context == Context::Shell && token == ":" {
            return next(context);
        }
        let first = token.chars().next().unwrap_or(' ');
        if NON_PROGRAM_PREFIX_CHARACTERS.contains(first)
            && !(token.starts_with('$') && token.contains('/'))
        {
            return if context == Context::Shell && token.starts_with('[') {
                next(context)
            } else {
                None
            };
        }
        let program = token.rsplit(['/', '\\']).next().unwrap_or(token);
        let unqualified = token == program;
        if program == "env" || program == "sudo" {
            wrapper = Some(if program == "env" {
                Wrapper::Env
            } else {
                Wrapper::Sudo
            });
            context = Context::Exec;
            index += 1;
            continue;
        }
        if wrapper.is_some() && token == "--" {
            wrapper = None;
            index += 1;
            continue;
        }
        if let Some(w) = wrapper
            && token.starts_with('-')
        {
            if w == Wrapper::Env && (token == "-S" || token == "--split-string") {
                return tokens
                    .get(index + 1)
                    .and_then(|split| program_name_internal(split, depth + 1, context, segments));
            }
            if w == Wrapper::Env
                && let Some(split) = token.strip_prefix("--split-string=")
            {
                return program_name_internal(split, depth + 1, context, segments);
            }
            if wrapper_options_with_value(w).contains(&token) {
                tokens.get(index + 1)?;
                index += 2;
                continue;
            }
            if wrapper_flags(w).contains(&token) {
                index += 1;
                continue;
            }
            if let Some(equals) = token.find('=')
                && token.starts_with("--")
                && equals > 2
            {
                if !wrapper_options_with_value(w).contains(&&token[..equals]) {
                    return None;
                }
                index += 1;
                continue;
            }
            if token.len() > 2
                && token
                    .chars()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_alphabetic())
                && !token.starts_with("--")
            {
                let letters: Vec<char> = token[1..].chars().collect();
                let mut consumes = false;
                for (i, letter) in letters.iter().enumerate() {
                    let short = format!("-{letter}");
                    if wrapper_options_with_value(w).contains(&short.as_str()) {
                        consumes = i == letters.len() - 1;
                        break;
                    }
                    if !wrapper_flags(w).contains(&short.as_str()) {
                        return None;
                    }
                }
                if consumes && tokens.get(index + 1).is_none() {
                    return None;
                }
                index += if consumes { 2 } else { 1 };
                continue;
            }
            return None;
        }
        let lower = program.to_lowercase();
        let shell = lower.strip_suffix(".exe").unwrap_or(&lower);
        if SHELL_PROGRAMS.contains(&shell)
            && let Some(script) = shell_command_argument_index(&tokens, index + 1)
        {
            return tokens
                .get(script)
                .and_then(|s| program_name_internal(s, depth + 1, Context::Shell, segments));
        }
        if WINDOWS_SHELL_PROGRAMS.contains(&lower.as_str())
            && let Some(payload) = windows_payload_program(
                &lower,
                &tokens,
                index + 1,
                depth,
                remaining,
                split.separator.as_deref(),
                segments,
            )
        {
            return payload;
        }
        if lower == "start-process"
            && let Some(started) = start_process_program(&tokens, index + 1)
        {
            return Some(started);
        }
        if context == Context::Shell && unqualified && SHELL_PRECOMMAND_MODIFIERS.contains(&program)
        {
            index += 1;
            if program == "time" && tokens.get(index).map(String::as_str) == Some("-p") {
                index += 1;
            }
            if tokens.get(index).is_some_and(|t| t.starts_with('-')) {
                return None;
            }
            continue;
        }
        if unqualified
            && let Some(target) = transparent_wrapper_index(program, &tokens, index)
            && let Some(wrapped) = program_name_internal(
                &serialize_tokens(&tokens[target..]),
                depth + 1,
                Context::Exec,
                segments,
            )
        {
            return Some(wrapped);
        }
        if unqualified && SHELL_COMMAND_WRAPPERS.contains(&program) {
            return wrapped_shell_program(program, &tokens, index + 1, depth, remaining, segments);
        }
        let skippable = (context == Context::Shell
            || (wrapper == Some(Wrapper::Sudo) && SKIPPABLE_SUDO_PROBES.contains(&program)))
            && unqualified
            && NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&program)
            && (!TERMINAL_SHELL_PROGRAMS.contains(&program)
                || (program == "false" && split.separator.as_deref() != Some("&&")));
        if skippable && remaining.is_some() {
            return next(Context::Shell);
        }
        if context == Context::Shell
            && unqualified
            && POWERSHELL_SETUP_PROGRAMS.contains(&lower.as_str())
            && remaining.is_some()
        {
            return next(Context::Shell);
        }
        let first = program.chars().next().unwrap_or(' ');
        let last = program.chars().last().unwrap_or(' ');
        if program.is_empty()
            || (unqualified && NON_DESCRIPTIVE_SHELL_PROGRAMS.contains(&program))
            || NON_PROGRAM_PREFIX_CHARACTERS.contains(first)
            || NON_PROGRAM_SUFFIX_CHARACTERS.contains(last)
            || program.ends_with("()")
            || FUNCTION_TOKEN.is_match(program)
        {
            return None;
        }
        return Some(program.to_owned());
    }
    if (saw_assignment || saw_redirection) && wrapper.is_none() {
        return next(context);
    }
    None
}

fn program_name_internal(
    command: &str,
    depth: usize,
    context: Context,
    segments: usize,
) -> Option<String> {
    if segments == 0 {
        return None;
    }
    if let Some(called) = powershell_call_program(command) {
        return called;
    }
    parse_program(command, depth, context, segments).or_else(|| {
        if context == Context::Shell {
            literal_alias_program(command)
        } else {
            None
        }
    })
}

/// `commandProgramName`: the program a command line runs, or `None` when it can't be told.
/// Rows ask on every frame, so answers are kept per command.
pub(crate) fn program_name(command: &str) -> Option<String> {
    thread_local! {
        static CACHE: std::cell::RefCell<std::collections::HashMap<String, Option<String>>> =
            Default::default();
    }
    if let Some(known) = CACHE.with(|c| c.borrow().get(command).cloned()) {
        return known;
    }
    let program = program_name_internal(command, 0, Context::Shell, MAX_COMMAND_SEGMENTS);
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.len() >= 512 {
            cache.clear();
        }
        cache.insert(command.to_owned(), program.clone());
    });
    program
}

/// `withVisibleControlCharacters`: control bytes as Unicode control pictures (ESC as ␛).
pub(crate) fn visible_controls(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\t' | '\n' | '\r' => c,
            '\u{7f}' => '␡',
            c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32).unwrap_or(c),
            c => c,
        })
        .collect()
}

/// `commandDisplayText`.
pub(crate) fn display_text(command: &str) -> String {
    visible_controls(&without_shell_wrapper(command))
}

/// `withoutShellWrapper`: a wrapper whose quoting doesn't balance is kept, since where its
/// script ends is unknown (Codex's redaction can eat an escaped quote's backslash).
fn without_shell_wrapper(command: &str) -> String {
    let trimmed = command.trim();
    if split_first(trimmed).remaining.is_some() {
        return trimmed.to_owned();
    }
    let Some(tokens) = tokenize(trimmed) else {
        return trimmed.to_owned();
    };
    let Some(executable) = tokens.first().and_then(|t| t.rsplit(['/', '\\']).next()) else {
        return trimmed.to_owned();
    };
    let program = executable
        .strip_suffix(".exe")
        .or_else(|| executable.strip_suffix(".EXE"))
        .unwrap_or(executable);
    let index = if SHELL_PROGRAMS.contains(&program) {
        shell_command_argument_index(&tokens, 1)
    } else if WINDOWS_SHELL_PROGRAMS.contains(&executable.to_lowercase().as_str()) {
        windows_script_index(&program.to_lowercase(), &tokens)
    } else {
        None
    };
    match index {
        Some(i) if i == tokens.len() - 1 => {
            let script = tokens[i].trim();
            if script.is_empty() {
                trimmed.to_owned()
            } else {
                script.to_owned()
            }
        }
        _ => trimmed.to_owned(),
    }
}

/// `windowsShellScriptIndex`.
fn windows_script_index(shell: &str, tokens: &[String]) -> Option<usize> {
    let mut i = 1;
    while i < tokens.len() {
        let option = tokens[i].to_lowercase();
        if shell == "cmd" {
            if option == "/c" {
                return Some(i + 1);
            }
            if !option.starts_with('/') {
                return None;
            }
        } else if option == "-command" || option == "-c" {
            return Some(i + 1);
        } else if POWERSHELL_OPTIONS_WITH_VALUE.contains(&option.as_str()) {
            i += 1;
        } else if !POWERSHELL_FLAGS.contains(&option.as_str()) {
            return None;
        }
        i += 1;
    }
    None
}

/// `commandHighlightLanguage`: PowerShell scripts highlight as PowerShell, others as shell.
pub(crate) fn highlight_language(command: &str) -> &'static str {
    static PROGRAM: LazyLock<Regex> =
        LazyLock::new(|| re(r#"^(?:&\s*)?(?:"([^"]*)"|'([^']*)'|(\S+))"#));
    let name = PROGRAM
        .captures(command.trim())
        .and_then(|c| c.get(1).or(c.get(2)).or(c.get(3)))
        .map(|m| {
            m.as_str()
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or("")
                .to_lowercase()
        })
        .unwrap_or_default();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    if name == "powershell" || name == "pwsh" || is_powershell_script(command) {
        "powershell"
    } else {
        "bash"
    }
}

const POWERSHELL_VERBS: &str = "Add Approve Assert Backup Block Build Checkpoint Clear Close Compare Complete Compress Confirm Connect Convert ConvertFrom ConvertTo Copy Debug Deny Deploy Disable Disconnect Dismount Edit Enable Enter Exit Expand Export Find ForEach Format Get Grant Group Hide Import Initialize Install Invoke Join Limit Lock Measure Merge Mount Move New Open Optimize Out Ping Pop Protect Publish Push Read Receive Redo Register Remove Rename Repair Request Reset Resize Resolve Restart Restore Resume Revoke Save Search Select Send Set Show Skip Sort Split Start Step Stop Submit Suspend Switch Sync Tee Test Trace Unblock Undo Uninstall Unlock Unprotect Unpublish Unregister Update Use Wait Watch Where Write";

fn is_powershell_cmdlet(word: &str) -> bool {
    static CMDLET: LazyLock<Regex> =
        LazyLock::new(|| re(r"^([A-Z][a-z]+(?:[A-Z][a-z]+)?)-[A-Z][A-Za-z]*$"));
    CMDLET
        .captures(word)
        .is_some_and(|c| POWERSHELL_VERBS.split(' ').any(|v| v == &c[1]))
}

fn is_powershell_script(command: &str) -> bool {
    static CALL: LazyLock<Regex> = LazyLock::new(|| re(r"^&\s*\S"));
    static SCOPE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)^\$(?:env|global|local|script):"));
    static VARIABLE: LazyLock<Regex> = LazyLock::new(|| re(r"^\$[A-Za-z_]\w*\s+="));
    let mut rest = Some(command.trim().to_owned());
    if CALL.is_match(rest.as_deref().unwrap_or("")) {
        return true;
    }
    for _ in 0..MAX_COMMAND_SEGMENTS {
        let Some(current) = rest.filter(|r| !r.is_empty()) else {
            break;
        };
        let split = split_first(&current);
        let statement = split.first.trim();
        if SCOPE.is_match(statement)
            || VARIABLE.is_match(statement)
            || is_powershell_cmdlet(statement.split_whitespace().next().unwrap_or(""))
        {
            return true;
        }
        rest = split.remaining;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{display_text, program_name};

    fn check(cases: &[(&str, Option<&str>)]) {
        for (command, program) in cases {
            assert_eq!(
                program_name(command).as_deref(),
                *program,
                "commandProgramName({command:?})"
            );
        }
    }

    #[test]
    fn matches_the_web_program_names() {
        check(&[
            (
                "/bin/bash --rcfile /tmp/config -c 'git status'",
                Some("git"),
            ),
            (
                "zsh -lc 'CI=1 env -u DEBUG sudo -u root vp test run'",
                Some("vp"),
            ),
            (
                r#""C:\Program Files\Git\bin\bash.exe" -lc "git status""#,
                Some("git"),
            ),
            (
                "/bin/zsh -lc 'git status\nsed -n '\"'1,20p' apps/web/src/components/DiffPanel.tsx\"",
                Some("git"),
            ),
            ("sudo -u root pnpm test", Some("pnpm")),
            ("zsh script.sh -c 'git status'", Some("zsh")),
            ("my-shell -c 'git status'", Some("my-shell")),
            ("$HOME/.bun/bin/bun test", Some("bun")),
            ("zsh -lc", None),
            ("zsh -lc 'git status", None),
            ("zsh -lc 'env'", None),
            ("if test -f package.json; then vp test; fi", None),
            ("[[ -f package.json ]]", None),
            ("test -f package.json", None),
            ("for file in *; do echo \"$file\"; done", None),
            ("check() { vp test; }", None),
            ("{ vp test; }", None),
            ("(( count += 1 ))", None),
            (". ./script.sh", None),
            ("export NODE_ENV=test", None),
            ("repeat 3 echo ok", None),
            ("return 1", None),
            ("true", None),
            ("rg -n \"if|for|while\" src", Some("rg")),
            ("vp test && git status", Some("vp")),
            ("echo `git status`", Some("echo")),
            ("cat <(rg needle src)", Some("cat")),
            ("cd packages/client-runtime && vp test run", Some("vp")),
            ("PATH+=:/tools npm test", Some("npm")),
            ("cd $(find . -type d | head -1) && git status", Some("git")),
            (
                "cd /tmp && # use the selected workspace\nnpm test",
                Some("npm"),
            ),
            ("cd ${ROOT:-path;with;semicolons} && bun test", Some("bun")),
            (
                "cd /path/to/repo && eval \"$(fnm env --shell zsh)\" && fnm use 26.13.1 && git commit -m \"msg\"",
                Some("fnm"),
            ),
            (
                "eval \"$(/opt/homebrew/bin/brew shellenv)\" && pnpm test",
                Some("pnpm"),
            ),
            ("source ~/.nvm/nvm.sh && nvm use", Some("nvm")),
            ("unset DEBUG; node app.js", Some("node")),
            (
                "/bin/zsh -lc 'source ~/.nvm/nvm.sh && nvm use'",
                Some("nvm"),
            ),
            ("timeout 10s python3 script.py", Some("python3")),
            ("arch -arch arm64 /opt/tools/check", Some("check")),
            (
                "timeout 60 script -q /dev/null env CI=1 node app.js",
                Some("node"),
            ),
            ("command -v git", None),
            ("exec > output.log", None),
            ("env CI=1 cd /tmp && npm test", None),
            ("$PY scripts/check.py", None),
            ("& { Get-Process }", None),
            ("coproc npm test", None),
            ("hash --help", Some("hash")),
            ("Exec node app.js", Some("Exec")),
            ("CD /tmp && npm test", Some("CD")),
            (
                "# first comment\n  # second comment\ngit status",
                Some("git"),
            ),
            (">/tmp/log && npm test", Some("npm")),
            ("cd /tmp && 2>& 1 node app.js", Some("node")),
            ("cd /tmp && {output}>/tmp/log deno test", Some("deno")),
            (
                "cd /tmp <<-EOF\n\tnot-a-command\n\tEOF\nbun test",
                Some("bun"),
            ),
            ("cd /tmp <<A <<B\none\nA\ntwo\nB\ngit status", Some("git")),
            (
                "cd /tmp <<EOF && pnpm test\nnot-a-command\nEOF\nbun test",
                Some("pnpm"),
            ),
        ]);
    }

    #[test]
    fn bounds_nesting_and_segments() {
        let mut command = "git status".to_owned();
        for _ in 0..9 {
            command = format!("sh -c '{}'", command.replace('\'', "'\\''"));
        }
        assert_eq!(program_name(&command), None);
        let mut many: Vec<String> = (0..2_000)
            .map(|i| format!("export VALUE_{i}=configured"))
            .collect();
        many.push("npm test".into());
        assert_eq!(program_name(&many.join(";")), None);
    }

    #[test]
    fn matches_the_web_display_text() {
        let cases = [
            (
                r#"/bin/zsh -lc 'curl -H "Authorization: Bearer [REDACTED_SECRET]" https://example.com'"#,
                r#"curl -H "Authorization: Bearer [REDACTED_SECRET]" https://example.com"#,
            ),
            (
                r#"/bin/zsh -lc "python3 -c 'token=[REDACTED_SECRET]"apiToken\"]; print(token)'""#,
                r#"/bin/zsh -lc "python3 -c 'token=[REDACTED_SECRET]"apiToken\"]; print(token)'""#,
            ),
            (
                r#""C:\Program Files\PowerShell\7\pwsh.exe" -Command "Get-Date -Format 'yyyy-MM-dd HH:mm:ss'""#,
                "Get-Date -Format 'yyyy-MM-dd HH:mm:ss'",
            ),
            (
                r#""C:\Program Files\PowerShell\7\pwsh.exe" -NoLogo -NoProfile -Command 'rg -n -F "new Date()" .'"#,
                r#"rg -n -F "new Date()" ."#,
            ),
            (r#"cmd.exe /c "npm test""#, "npm test"),
            ("printf '\u{1b}[31mred\u{1b}[0m'", "printf '␛[31mred␛[0m'"),
            ("zsh -lc 'printf \"\u{1b}[1m\"'", "printf \"␛[1m\""),
            ("printf 'a\\tb'\r\necho done", "printf 'a\\tb'\r\necho done"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                display_text(input),
                expected,
                "commandDisplayText({input:?})"
            );
        }
    }
}
