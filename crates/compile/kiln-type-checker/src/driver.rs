//! One project's check, and its diagnostics as TypeScript reports them: Bun's
//! `bun_sema_driver::check_project` (`src/sema/driver/lib.rs`) for a single project, without its
//! command line, build mode, progress and plan instrumentation.
//!
//! Every diagnostic a program has comes from here: configuration errors, option and program
//! problems, the errors of each file with their related information and message chains, global
//! errors, declaration emit errors and unreadable files, sorted and deduplicated as
//! `SortAndDeduplicateDiagnostics` does. The source checker and the TypeScript conformance runner
//! both check through it, so the second measures the first.

use crate::engine::check::errors::Checked;
use crate::engine::check::explain::Explained;
use crate::engine::check::task::Finished;
use crate::engine::check::{Checker, Program, Requested, compute_ecma_line_starts};
use crate::engine::config::{ConfigError, Project};
use crate::engine::hir::{ExprTag, FileKind};
use crate::engine::messages;
use crate::engine::program::{FileId, Files, IN_CONFIGURATION};
use crate::engine::resolve::{Host, displayed_path, typescript_path};
use crate::engine::session::Session;
use std::sync::Mutex;

pub use crate::engine::check::explain::MessageChain;
pub use crate::engine::messages::Category;

/// Lines of the source shown before and after a diagnostic.
const LINES_BEFORE: u32 = 3;
const LINES_AFTER: u32 = 2;

/// A diagnostic, ready to be formatted.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    /// The file, in the checker's path format. Empty for a configuration error.
    pub path: Vec<u8>,
    /// Offsets in bytes.
    pub start: u32,
    /// Before `start` for a missing value in a JSON file, which decides the order.
    pub end: u32,
    /// `Loc` is `UndefinedTextRange`, which comes before offset 0: `NewCompilerDiagnostic`.
    pub has_undefined_range: bool,
    /// 1-based. Columns count UTF-16 code units, as TypeScript's do.
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub code: u32,
    pub category: Category,
    /// The message. Lines after the first are the elaboration, indented by two spaces per level.
    pub text: Vec<u8>,
    /// `MessageArgs` and `MessageChain`, which `text` is made of.
    pub args: Box<[Box<[u8]>]>,
    pub message_chain: Vec<MessageChain>,
    /// Lines of the file from `source_line` on, without their line terminators: a few before the
    /// error, the lines it spans, a few after.
    pub source: Vec<Vec<u8>>,
    pub source_line: u32,
    /// Related information: `'x' is declared here.` These entries have no related information of
    /// their own.
    pub related: Vec<Diagnostic>,
}

#[derive(Default)]
pub struct Report {
    /// Sorted as TypeScript sorts them: diagnostics without a file first, then by path and
    /// position.
    pub diagnostics: Vec<Diagnostic>,
    /// Files in which a query was abandoned because the stack ran out: errors may be missing.
    pub incomplete: Vec<Vec<u8>>,
    /// What `traceResolution` logs.
    pub resolution_trace: Vec<Vec<u8>>,
    /// The path and the text of each declaration file emitted.
    pub declaration_files: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Called with a checker and each file it has checked, while what it computed is alive.
pub type AfterFile<'a> = &'a (dyn Fn(&mut Checker<'_, '_>, FileId) + Sync);

/// Called with the program once it is checked, before it is freed, and the report so far.
pub type AfterCheck<'a> = &'a (dyn Fn(&Program<'_>, &Report) + Sync);

/// How a project is checked.
#[derive(Default)]
pub struct Request<'a> {
    /// `GetDiagnosticsOfAnyProgram`: stop after the first kind of diagnostic there is any of, as
    /// `tsc` does. Otherwise every kind is collected, as TypeScript's test harness does.
    pub stops_like_tsc: bool,
    /// Keep every type a file's check computed, for queries after the check.
    pub retains_everything: bool,
    /// Called after each file is checked, and once for each file that is not.
    pub after_file: Option<AfterFile<'a>>,
    pub checked: Option<AfterCheck<'a>>,
}

/// `NewCompilerDiagnostic`
fn global(code: u32, args: &[impl AsRef<[u8]>]) -> Diagnostic {
    let args: Box<[Box<[u8]>]> = args.iter().map(|arg| arg.as_ref().into()).collect();
    let (category, template) =
        messages::message(code).unwrap_or((Category::Error, "Unknown error."));
    let mut text = Vec::new();
    messages::format(&mut text, template, &args);
    Diagnostic {
        path: Vec::new(),
        start: 0,
        end: 0,
        has_undefined_range: true,
        line: 0,
        column: 0,
        end_line: 0,
        end_column: 0,
        code,
        category,
        text,
        args,
        message_chain: Vec::new(),
        source: Vec::new(),
        source_line: 0,
        related: Vec::new(),
    }
}

/// `ConfigError::chain` as `MessageChain`: each element is under the last one of the level above.
fn message_chain_of(lines: &[(u32, u32, Vec<Vec<u8>>)]) -> Vec<MessageChain> {
    let mut chain: Vec<MessageChain> = Vec::new();
    for (level, code, args) in lines {
        let mut under = &mut chain;
        for _ in 1..*level {
            if under.is_empty() {
                break;
            }
            under = &mut under.last_mut().unwrap().message_chain;
        }
        under.push(MessageChain {
            code: *code,
            args: args.iter().map(|arg| arg.as_slice().into()).collect(),
            message_chain: Vec::new(),
        });
    }
    chain
}

/// `reported`, located at the bytes `start..end` of the file at `path`, whose text is `text` and
/// whose line starts are `starts`.
fn located(
    path: &[u8],
    text: &[u8],
    starts: &[u32],
    start: u32,
    end: u32,
    reported: Diagnostic,
) -> Diagnostic {
    let (line, character) = line_and_character(text, starts, start);
    let (end_line, end_character) = line_and_character(text, starts, end.max(start));
    let source_line = line.saturating_sub(LINES_BEFORE);
    let last_line = (end_line + LINES_AFTER).min(starts.len() as u32 - 1);
    Diagnostic {
        path: path.to_vec(),
        start,
        end,
        has_undefined_range: false,
        line: line + 1,
        column: character + 1,
        end_line: end_line + 1,
        end_column: end_character + 1,
        source: (source_line..=last_line)
            .map(|l| line_text(text, starts, l))
            .collect(),
        source_line: source_line + 1,
        ..reported
    }
}

/// `GetECMALineAndUTF16CharacterOfPosition`: the 0-based line that contains `offset`, and the
/// number of UTF-16 code units before it on that line.
pub fn line_and_character(text: &[u8], starts: &[u32], offset: u32) -> (u32, u32) {
    let offset = offset.min(text.len() as u32);
    let line = starts.partition_point(|&s| s <= offset) - 1;
    let before = &text[starts[line] as usize..offset as usize];
    (line as u32, utf16_len(before) as u32)
}

/// `core.UTF16Len`: a byte that is not part of a well-formed sequence is a U+FFFD of its own.
pub fn utf16_len(text: &[u8]) -> usize {
    let Some(ascii) = text.iter().position(|b| !b.is_ascii()) else {
        return text.len();
    };
    let (mut units, mut rest) = (ascii, &text[ascii..]);
    while !rest.is_empty() {
        let (rune, size) = crate::engine::check::regexp_scanner::decode_rune(rest);
        units += if rune < 0x10000 { 1 } else { 2 };
        rest = &rest[size..];
    }
    units
}

fn line_text(text: &[u8], starts: &[u32], line: u32) -> Vec<u8> {
    let from = starts[line as usize] as usize;
    let to = starts
        .get(line as usize + 1)
        .map_or(text.len(), |&s| s as usize);
    let mut line = &text[from..to];
    loop {
        line = if let Some(rest) = line.strip_suffix(b"\n").or(line.strip_suffix(b"\r")) {
            rest
        } else if let Some(rest) = line
            .strip_suffix("\u{2028}".as_bytes())
            .or(line.strip_suffix("\u{2029}".as_bytes()))
        {
            rest
        } else {
            return line.to_vec();
        };
    }
}

/// `Loc`
fn loc(d: &Diagnostic) -> (i64, i64) {
    match d.has_undefined_range {
        true => (-1, -1),
        false => (i64::from(d.start), i64::from(d.end)),
    }
}

/// `EqualDiagnostics`
fn equal_diagnostics(d1: &Diagnostic, d2: &Diagnostic) -> bool {
    equal_diagnostics_no_related_info(d1, d2)
        && d1.related.len() == d2.related.len()
        && (d1.related.iter().zip(&d2.related)).all(|(r1, r2)| equal_diagnostics(r1, r2))
}

/// `EqualDiagnosticsNoRelatedInfo`
fn equal_diagnostics_no_related_info(d1: &Diagnostic, d2: &Diagnostic) -> bool {
    (&d1.path, loc(d1), d1.code, &d1.args) == (&d2.path, loc(d2), d2.code, &d2.args)
        && d1.message_chain == d2.message_chain
}

/// `compareMessageChainSize`
fn compare_message_chain_size(c1: &[MessageChain], c2: &[MessageChain]) -> std::cmp::Ordering {
    let by_length = c2.len().cmp(&c1.len());
    c1.iter().zip(c2).fold(by_length, |c, (c1, c2)| {
        c.then_with(|| compare_message_chain_size(&c1.message_chain, &c2.message_chain))
    })
}

/// `compareMessageChainContent`, of chains of the same size.
fn compare_message_chain_content(c1: &[MessageChain], c2: &[MessageChain]) -> std::cmp::Ordering {
    let equal = std::cmp::Ordering::Equal;
    c1.iter().zip(c2).fold(equal, |c, (c1, c2)| {
        c.then_with(|| c1.args.cmp(&c2.args))
            .then_with(|| compare_message_chain_content(&c1.message_chain, &c2.message_chain))
    })
}

/// `compareRelatedInfo`
fn compare_related_info(r1: &[Diagnostic], r2: &[Diagnostic]) -> std::cmp::Ordering {
    let by_length = r2.len().cmp(&r1.len());
    r1.iter().zip(r2).fold(by_length, |c, (d1, d2)| {
        c.then_with(|| compare_diagnostics(d1, d2))
    })
}

/// `CompareDiagnostics`
fn compare_diagnostics(d1: &Diagnostic, d2: &Diagnostic) -> std::cmp::Ordering {
    let (path1, path2) = (typescript_path(&d1.path), typescript_path(&d2.path));
    (path1, loc(d1), d1.code, &d1.args)
        .cmp(&(path2, loc(d2), d2.code, &d2.args))
        .then_with(|| compare_message_chain_size(&d1.message_chain, &d2.message_chain))
        .then_with(|| compare_message_chain_content(&d1.message_chain, &d2.message_chain))
        .then_with(|| compare_related_info(&d1.related, &d2.related))
}

/// `SortAndDeduplicateDiagnostics`
pub fn sort_and_deduplicate(diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by(compare_diagnostics);
    // `compactAndMergeRelatedInfos`
    diagnostics.dedup_by(|next, first| {
        let is_same = equal_diagnostics_no_related_info(first, next);
        if is_same {
            first.related.append(&mut next.related);
            first.related.sort_by(compare_diagnostics);
            first
                .related
                .dedup_by(|next, first| equal_diagnostics(first, next));
        }
        is_same
    });
}

/// `error`, of the project whose configuration file is at `config_path`.
pub fn of_config_error(host: &dyn Host, config_path: &[u8], error: &ConfigError) -> Diagnostic {
    let mut reported = global(error.code, &error.args);
    reported.message_chain = message_chain_of(&error.chain);
    for chain in &reported.message_chain {
        chain.flatten(&mut reported.text, 1);
    }
    if error.at.is_none() && error.related.is_empty() {
        return reported;
    }
    let path = error.at.as_ref().map_or(config_path, |at| &at.0[..]);
    let Some(text) = host.read(path) else {
        return reported;
    };
    let starts = compute_ecma_line_starts(&text);
    let related = error.related.iter().map(|(from, to, code, args)| {
        located(path, &text, &starts, *from, *to, global(*code, args))
    });
    reported.related = related.collect();
    match error.at {
        Some((_, from, to)) => located(path, &text, &starts, from, to, reported),
        None => reported,
    }
}

/// Checks `project`, which is read through `host`.
pub fn check_project(host: &dyn Host, mut project: Project, request: &Request) -> Report {
    let mut report = Report::default();
    let config_path = project.config_path.clone();
    let of_configuration = |error: &ConfigError| of_config_error(host, &config_path, error);
    // `GetDiagnosticsOfAnyProgram`: configuration file parsing errors are reported regardless of
    // any other diagnostics.
    report.diagnostics.extend(
        project
            .errors
            .iter()
            .filter(|error| !error.is_about_options)
            .map(of_configuration),
    );
    let always_reported = report.diagnostics.len();
    let mut about_options: Vec<Diagnostic> = project
        .errors
        .iter()
        .filter(|error| error.is_about_options)
        .map(of_configuration)
        .collect();
    let (skip_lib_check, skip_default_lib_check) = (
        project.options.skip_lib_check,
        project.options.skip_default_lib_check,
    );
    project.options.drops_unreferenced = !request.retains_everything;
    // Declared before everything that is allocated in it, so it is dropped last.
    let session = Session::new();
    let files = Files::load(&session, host, project.options, &project.files);
    host.loaded();
    let files = session.arena().alloc(files);
    let root = session.arena().alloc(Program::new(&session, files));
    let program: &Program = root;
    let trace = program.files.resolution_trace.iter();
    report.resolution_trace = trace.map(|it| global(it.code, &it.args).text).collect();
    about_options.extend(
        program
            .files
            .program_problems()
            .iter()
            .map(|problem| ConfigError::of_problem(host, &session, &config_path, problem))
            .map(|error| of_configuration(&error)),
    );

    let mut to_check: Vec<FileId> = (0..program.files.modules.len())
        .filter(|&i| {
            let module = &program.files.modules[i];
            match module.hir.kind {
                // Only parse errors are reported for JSON files.
                FileKind::Json => module.hir.has_parse_diagnostics,
                // `SkipTypeChecking`, and nothing is emitted for a declaration file.
                FileKind::Declaration => {
                    !skip_lib_check && !(skip_default_lib_check && module.is_lib)
                }
                FileKind::Ts | FileKind::Tsx => true,
            }
        })
        .map(|i| FileId(i as u32))
        .collect();
    // Program order (`program.files`): an imported file precedes its importers.
    to_check.sort_by_key(|&f| program.files.rank_of_file(f));
    // `SkipTypeChecking`: besides its syntax, such a file only has what `Emit` reports, which is
    // called after the semantic and the global diagnostics of the whole program are collected. So
    // these files come after the plan, or before it under `Options::emits_first`. If there are no
    // others, the plan is theirs.
    let checker = program.checker();
    let (type_checked, only_emitted): (Vec<FileId>, Vec<FileId>) =
        (to_check.into_iter()).partition(|&file| checker.reports_semantic_errors(file));
    drop(checker);
    let is_any_type_checked = !type_checked.is_empty();
    let (to_check, only_emitted) = match is_any_type_checked {
        true => (type_checked, only_emitted),
        false => (only_emitted, type_checked),
    };
    let found: Mutex<Vec<Diagnostic>> = Mutex::new(Vec::new());
    // `GetDeclarationDiagnostics`
    let emit_diagnostics: Mutex<Vec<Diagnostic>> = Mutex::new(Vec::new());
    let incomplete: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
    // `Files::parse_and_bind` retains the text of every file except those of the default library.
    let text_of = |file: FileId| {
        let module = &program.files.modules[file.idx()];
        if module.is_lib && module.hir.text.is_empty() {
            host.read(module.file_name()).unwrap_or_default()
        } else {
            std::borrow::Cow::Borrowed(&module.hir.text[..])
        }
    };
    let show = |file: FileId, errors: Vec<Explained>, found: &Mutex<Vec<Diagnostic>>| {
        if errors.is_empty() {
            return;
        }
        let module = &program.files.modules[file.idx()];
        let text = &text_of(file)[..];
        let starts = compute_ecma_line_starts(text);
        let shown: Vec<Diagnostic> = errors
            .into_iter()
            .map(|e| {
                let related = e
                    .related
                    .into_iter()
                    .map(|related| {
                        let reported = Diagnostic {
                            code: related.code,
                            category: related.category,
                            text: related.text,
                            args: related.args,
                            message_chain: related.message_chain,
                            ..global(0, &[""; 0])
                        };
                        let Some((of, start, end)) = related.at else {
                            return reported;
                        };
                        if of == file {
                            return located(
                                module.file_name(),
                                text,
                                &starts,
                                start,
                                end,
                                reported,
                            );
                        }
                        if of == IN_CONFIGURATION {
                            return match host.read(&config_path) {
                                Some(text) => {
                                    let starts = compute_ecma_line_starts(&text);
                                    located(&config_path, &text, &starts, start, end, reported)
                                }
                                None => reported,
                            };
                        }
                        let other = &program.files.modules[of.idx()];
                        let text = &text_of(of)[..];
                        located(
                            other.file_name(),
                            text,
                            &compute_ecma_line_starts(text),
                            start,
                            end,
                            reported,
                        )
                    })
                    .collect();
                let reported = Diagnostic {
                    related,
                    code: e.code,
                    category: e.category,
                    text: e.text,
                    args: e.args,
                    message_chain: e.message_chain,
                    ..global(0, &[""; 0])
                };
                located(module.file_name(), text, &starts, e.start, e.end, reported)
            })
            .collect();
        found.lock().unwrap().extend(shown);
    };
    // The files that are checked but not yet rendered. The task of another file may still report
    // diagnostics in them.
    let unfinished: Mutex<Vec<(FileId, Checked)>> = Mutex::new(Vec::new());
    let declaration_files: Mutex<Vec<(Vec<u8>, Vec<u8>)>> = Mutex::new(Vec::new());
    // `task`: its step, its index in the step, and `is_read_later`. `None`: the files are checked
    // outside the plan.
    let check_chunk =
        |files: &[FileId], expected: Requested, task: Option<(usize, usize, bool)>| {
            let mut checker = program.checker();
            checker.set_requested(expected);
            checker.begin_stack_budget();
            if let Some((step, index, is_read_later)) = task {
                checker.begin_task(step as u32, index as u32, is_read_later);
            }
            let mut checked = Vec::new();
            for &file in files {
                checked.push((file, checker.check_file(file)));
                if let Some(after_file) = request.after_file
                    && expected == Requested::All
                {
                    after_file(&mut checker, file);
                }
                if checker.take_ran_out_of_stack() {
                    let path = program.files.modules[file.idx()].file_name().to_vec();
                    incomplete.lock().unwrap().push(path);
                }
            }
            let finished = task.map(|_| checker.end_task());
            (finished, checked)
        };
    // The report. No task is running: every diagnostic is in the buffer of its file.
    let finish_files = || {
        let unfinished: Vec<Mutex<Option<(FileId, Checked)>>> = unfinished
            .lock()
            .unwrap()
            .drain(..)
            .map(|one| Mutex::new(Some(one)))
            .collect();
        host.parallel(unfinished.len(), &|i| {
            let (file, mut checked) = unfinished[i].lock().unwrap().take().unwrap();
            if let Some(written) = checked.declaration_file.take() {
                let path = program.files.modules[file.idx()].file_name().to_vec();
                declaration_files.lock().unwrap().push((path, written));
            }
            let declaration = checked.take_declaration_diagnostics();
            show(file, program.finish_file(file, checked), &found);
            if let Some(declaration) = declaration {
                let declaration = program.finish_file(file, declaration);
                show(file, declaration, &emit_diagnostics);
            }
        });
    };
    let bytes_of =
        |index: usize| program.files.modules[to_check[index].idx()].hir.source_len as usize;
    // The identifiers of a file's expressions and its type nodes: each is a symbol to resolve or a
    // type to compute (the donor's estimate).
    let costs: Vec<usize> = (to_check.iter())
        .map(|&file| {
            let hir = &program.files.modules[file.idx()].hir;
            let tags = hir.exprs.iter().map(|it| it.kind.tag());
            tags.filter(|&tag| tag == ExprTag::Ident).count() + hir.types.len() + 1
        })
        .collect();
    let cost_of = |index: usize| costs[index];
    let plan = crate::plan::Plan::new(to_check.len(), &bytes_of, &cost_of);
    let in_parallel = |count: usize, work: &(dyn Fn(usize) + Sync)| host.parallel(count, work);
    // Each step's tasks run at once, each a checker of its own; a barrier after them validates,
    // links and publishes them in task order. An invalid task's files are checked again in a
    // later round of the same step.
    let run_step = |number: usize, step: &[Vec<usize>], expected: Requested| {
        let is_read_later = number + 1 < plan.steps.len()
            || request.retains_everything
            || request.after_file.is_some()
            || request.checked.is_some();
        let mut tasks = step.to_vec();
        while !tasks.is_empty() {
            type Outcome<'s> = (Option<Finished<'s>>, Vec<(FileId, Checked)>);
            let outcomes: Vec<Mutex<Option<Outcome>>> =
                tasks.iter().map(|_| Mutex::new(None)).collect();
            host.parallel(tasks.len(), &|index| {
                let files: Vec<FileId> = tasks[index].iter().map(|&at| to_check[at]).collect();
                let outcome = check_chunk(&files, expected, Some((number, index, is_read_later)));
                *outcomes[index].lock().unwrap() = Some(outcome);
            });
            let (mut finished, mut checked): (Vec<Finished>, Vec<_>) = (outcomes.into_iter())
                .map(|outcome| {
                    let (finished, checked) = outcome.into_inner().unwrap().unwrap();
                    (finished.unwrap(), checked)
                })
                .unzip();
            let is_invalid = program.validate(&finished);
            let mut at = is_invalid.iter();
            finished.retain(|_| !*at.next().unwrap());
            let mut invalid = Vec::new();
            for ((checked, task), &is_invalid) in checked.drain(..).zip(&tasks).zip(&is_invalid) {
                if is_invalid {
                    invalid.extend(task.iter().copied());
                } else {
                    unfinished.lock().unwrap().extend(checked);
                }
            }
            program.link(&mut finished, &in_parallel);
            program.publish(&mut finished, &in_parallel, false);
            invalid.sort_unstable();
            tasks = crate::plan::Plan::cut(invalid, &cost_of);
        }
    };
    // `NewDiagnosticForNode` without a node: the range is empty, not undefined.
    let global_errors = || -> Vec<Diagnostic> {
        program
            .global_errors()
            .iter()
            .map(|(code, args)| Diagnostic {
                has_undefined_range: false,
                ..global(*code, args)
            })
            .collect()
    };
    // `GetDiagnosticsOfAnyProgram`: TypeScript's command line proceeds to the next kind of error
    // only if there is none of the previous kind. A file that does not parse, or is checked under
    // inconsistent options, produces errors that are not worth reading.
    let stops = request.stops_like_tsc;
    // `type_checked`: the files that are, or else the others.
    let check_files = |expected: Requested, type_checked: bool| {
        if type_checked == is_any_type_checked {
            for (number, step) in plan.steps.iter().enumerate() {
                run_step(number, step, expected);
            }
        }
        if !type_checked {
            let (_, checked) = check_chunk(&only_emitted, expected, None);
            unfinished.lock().unwrap().extend(checked);
        }
        finish_files();
    };
    // `EmitFilesAndReportErrors` calls `Emit` after collecting diagnostics, even if there are
    // errors, and the declaration transformer reports under `noEmit` too
    // (`emitDeclarationFile`). `HandleNoEmitOnError` skips `Emit`, and so does `noEmit` on an
    // incremental program. Otherwise this models `tsc -p --noEmit`.
    let options = &program.files.options;
    let emits_despite_errors = options.emits_declarations
        && !options.no_emit_on_error
        && !match options.is_build {
            true => options.no_emit,
            false => options.is_incremental,
        };
    let emit_on_early_exit = || -> Vec<Diagnostic> {
        if emits_despite_errors {
            check_files(Requested::Declaration, true);
            check_files(Requested::Declaration, false);
            found.lock().unwrap().clear();
        }
        std::mem::take(&mut *emit_diagnostics.lock().unwrap())
    };
    'stages: {
        if stops {
            let suspects: Vec<FileId> = (0..program.files.modules.len())
                .filter(|&i| {
                    let hir = &program.files.modules[i].hir;
                    hir.has_parse_diagnostics || hir.has_parse_or_grammar_diagnostics() || hir.is_js
                })
                .map(|i| FileId(i as u32))
                .collect();
            host.parallel(suspects.len(), &|i| {
                let (_, checked) = check_chunk(&suspects[i..=i], Requested::Syntactic, None);
                unfinished.lock().unwrap().extend(checked);
            });
            finish_files();
            let syntactic = std::mem::take(&mut *found.lock().unwrap());
            if !syntactic.is_empty() {
                report.diagnostics.extend(syntactic);
                report.diagnostics.extend(emit_on_early_exit());
                break 'stages;
            }
        }
        report.diagnostics.append(&mut about_options);
        if stops {
            report.diagnostics.extend(global_errors());
            if report.diagnostics.len() > always_reported {
                report.diagnostics.extend(emit_on_early_exit());
                break 'stages;
            }
        }
        let emits_first = options.emits_first && !options.no_emit_on_error;
        for type_checked in [!emits_first, emits_first] {
            check_files(Requested::All, type_checked);
            report.diagnostics.append(&mut found.lock().unwrap());
            // What a checker reports without a file after this is never asked for.
            if type_checked {
                report.diagnostics.extend(global_errors());
            }
        }
        // `GetDiagnosticsOfAnyProgram` collects them itself if there are no other errors. This
        // list also contains suggestions.
        let is_error = |d: &Diagnostic| d.category == Category::Error;
        if !stops
            || emits_despite_errors
            || !report.diagnostics[always_reported..].iter().any(is_error)
        {
            report
                .diagnostics
                .append(&mut emit_diagnostics.lock().unwrap());
        }
        // `iterateBaseline`: a caller that writes output for every file also does so for the
        // files that are not checked.
        if let Some(after_file) = request.after_file {
            let mut is_checked = vec![false; program.files.modules.len()];
            for file in to_check.iter().chain(&only_emitted) {
                is_checked[file.idx()] = true;
            }
            for i in 0..program.files.modules.len() {
                let (file, module) = (FileId(i as u32), &program.files.modules[i]);
                // None of TypeScript's own libraries is a file of the caller's. With
                // `libReplacement` a library can be any file.
                let is_left_out = match module.is_lib {
                    true => !options.lib_replacement,
                    false => !matches!(module.hir.kind, FileKind::Declaration | FileKind::Json),
                };
                if is_left_out || is_checked[i] {
                    continue;
                }
                let mut checker = program.checker();
                checker.begin_stack_budget();
                after_file(&mut checker, file);
            }
        }
    }
    report.incomplete = std::mem::take(&mut *incomplete.lock().unwrap());
    report.incomplete.sort();
    report.incomplete.dedup();
    let unreadable = host.take_unreadable();
    let cannot_read = (unreadable.iter()).map(|path| global(5083, &[displayed_path(path)]));
    report.diagnostics.extend(cannot_read);
    sort_and_deduplicate(&mut report.diagnostics);
    report.declaration_files = std::mem::take(&mut *declaration_files.lock().unwrap());
    if let Some(checked) = request.checked {
        checked(program, &report);
    }
    root.release();
    report
}
