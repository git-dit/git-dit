//   git-dit - the distributed issue tracker for git
//   Copyright (C) 2016, 2017 Matthias Beyer <mail@beyermatthias.de>
//   Copyright (C) 2016, 2017 Julian Ganz <neither@nut.email>
//
//   This program is free software; you can redistribute it and/or modify
//   it under the terms of the GNU General Public License version 2 as
//   published by the Free Software Foundation.
//

#[macro_use] extern crate clap;
#[macro_use] extern crate error_chain;
#[macro_use] extern crate is_match;
#[macro_use] extern crate lazy_static;
#[macro_use] extern crate log;
extern crate atty;
extern crate chrono;
extern crate git2;
extern crate libgitdit;
extern crate regex;

#[macro_use] mod display;

mod error;
mod filters;
mod gitext;
mod system;
mod util;

use clap::App;
use git2::Commit;
use libgitdit::issue::IssueRefType;
use libgitdit::message::LineIteratorExt;
use libgitdit::{Message, RepositoryExt};
use log::Level;
use std::fs::File;
use std::io::{self, Read, Write};

use util::{RepositoryUtil};
use system::{Abortable, IteratorExt, LinesExt};


// Plumbing subcommand implementations

/// check-message subcommand implementation
///
fn check_message(matches: &clap::ArgMatches) {
    let reader: Box<Read> = match matches.value_of("filename") {
        Some(filename)  => Box::from(File::open(filename).unwrap_or_abort()),
        None            => Box::from(io::stdin()),
    };
    use io::BufRead;
    io::BufReader::new(reader)
        .lines()
        .abort_on_err()
        .skip_while(|l| l.is_empty())
        .stripped()
        .check_message_format()
        .unwrap_or_abort();
}


/// check-message subcommand implementation
///
fn check_refname(matches: &clap::ArgMatches) {
    // NOTE: check-refname is always present since it is a required parameter
    let refdata = IssueRefType::of_ref(matches.value_of("refname").unwrap());
    if let Some((id, reftype)) = refdata {
        // The reference is a valid dit reference. We may now answer questions
        // about it.
        if matches.is_present("issue-id") {
            println!("{}", id);
        }
        if matches.is_present("reftype") {
            println!("{}", match reftype {
                IssueRefType::Head => "head",
                IssueRefType::Leaf => "leaf",
                _ => "unknown",
            });
        }
    } else {
        use std::process::exit;
        exit(1);
    }
}


/// create-message subcommand implementation
///
fn create_message(matches: &clap::ArgMatches) {
    let repo = util::open_dit_repo();
    let issue = repo.cli_issue(matches);
    let author = repo.cli_author(matches);
    let committer = repo.signature().unwrap_or_abort();

    // Note: The list of parents must live long enough to back the references we
    //       supply to `libgitdit::repository::RepositoryExt::create_message()`.
    let parents = matches.values_of("parents")
                         .map(|p| repo.values_to_commits(p))
                         .unwrap_or_default();
    let parent_refs = parents.iter().map(|command| command);

    // use the first parent's tree if availible
    let tree = match parents.first() {
        Some(commit) => commit.tree().unwrap_or_abort(),
        _            => repo.empty_tree().unwrap_or_abort(),
    };

    // read all from stdin
    let mut message = String::new();
    io::stdin().read_to_string(&mut message).unwrap_or_abort();
    let id = match issue {
        Some(i) => i.add_message(&author, &committer, message, &tree, parent_refs)
                    .unwrap_or_abort()
                    .id(),
        None => repo.create_issue(&author, &committer, message, &tree, parent_refs)
                    .unwrap_or_abort()
                    .id(),
    };

    println!("{}", id);
}


/// find-tree-init-hash subcommand implementation
///
fn find_tree_init_hash(matches: &clap::ArgMatches) {
    let repo = util::open_dit_repo();

    // note: commit is always present since it is a required parameter
    let commit = repo.value_to_commit(matches.value_of("commit").unwrap());
    println!("{}", repo.issue_with_message(&commit).unwrap_or_abort());
}


/// get-issue-metadata subcommand implementation
///
fn get_issue_metadata(matches: &clap::ArgMatches) {
    use libgitdit::trailer::accumulation::{self, Accumulator};
    use libgitdit::trailer::iter::PairsToTrailers;

    let repo = util::open_dit_repo();

    // note: "head" is always present since it is a required parameter
    let head = repo.value_to_commit(matches.value_of("head").unwrap());
    let trailers = repo
        .issue_messages_iter(head)
        .abort_on_err()
        .flat_map(|commit| commit.trailers());

    if let Some(key) = matches.value_of("key") {
        let policy = if matches.is_present("accumulate-latest") {
            accumulation::AccumulationPolicy::Latest
        } else if matches.is_present("accumulate-list") {
            accumulation::AccumulationPolicy::List
        } else {
            accumulation::AccumulationPolicy::List
        };
        let mut acc = accumulation::SingleAccumulator::new(key.to_owned(), policy);
        acc.process_all(trailers);
        if matches.is_present("values-only") {
            acc.into_values().print_lines().unwrap_or_abort();
        } else {
            PairsToTrailers::from(acc).print_lines().unwrap_or_abort();
        }
    } else {
        trailers.print_lines().unwrap_or_abort();
    }
}


/// find-tree-init-hash subcommand implementation
///
fn get_issue_tree_init_hashes(_: &clap::ArgMatches) {
    let repo = util::open_dit_repo();

    repo.issues().unwrap_or_abort().print_lines().unwrap_or_abort();
}


// Porcelain subcommand implementations

/// fetch subcommand implementation
///
fn fetch_impl(matches: &clap::ArgMatches) {
    use libgitdit::RemoteExt;

    let repo = util::open_dit_repo();

    // note: "remote" is always present since it is a required parameter
    let mut remote = repo
        .find_remote(matches.value_of("remote").unwrap())
        .unwrap_or_abort();

    // accumulate the refspecs to fetch
    let refspecs : Vec<String> = if let Some(mut issues) = repo.cli_issues(matches) {
        // fetch a specific list of issues
        if matches.is_present("known") {
            issues.extend(repo.issues().unwrap_or_abort());
        }
        issues
            .into_iter()
            .filter_map(|issue| remote.issue_refspec(issue))
            .collect()
    } else {
        vec![remote.all_issues_refspec().unwrap()]
    };

    // set the options for the fetch
    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.prune(if matches.is_present("prune") {
        git2::FetchPrune::On
    } else {
        git2::FetchPrune::Unspecified
    });
    fetch_options.remote_callbacks(gitext::callbacks());

    let refspec_refs : Vec<&str> = refspecs.iter().map(String::as_str).collect();
    remote.fetch(refspec_refs.as_ref(), Some(&mut fetch_options), None)
          .unwrap_or_abort();
}


/// gc subcommand implementation
///
fn gc_impl(matches: &clap::ArgMatches) {
    use libgitdit::gc::ReferenceCollectionSpec;
    use libgitdit::iter::ReferenceDeletingIter;

    let repo = util::open_dit_repo();

    let collect = {
        let collect_heads = if matches.is_present("collect-heads") {
            ReferenceCollectionSpec::BackedByRemoteHead
        } else {
            ReferenceCollectionSpec::Never
        };
        repo.collectable_refs()
            .consider_remote_refs(matches.is_present("consider-remote"))
            .collect_heads(collect_heads)
    };

    let refs = repo
        .cli_issues(matches)
        .unwrap_or_else(|| repo.issues().unwrap_or_abort())
        .into_iter()
        .map(|issue| collect.for_issue(&issue))
        .abort_on_err()
        .flat_map(|collector| collector)
        .abort_on_err();

    if matches.is_present("dry-run") {
        refs.into_iter()
            .map(|r| r.name().unwrap_or("Unknown ref").to_owned())
            .print_lines()
            .unwrap_or_abort();
    } else {
        ReferenceDeletingIter::from(refs).print_lines().unwrap_or_abort();
    }
}


/// list subcommand implementation
///
fn list_impl(matches: &clap::ArgMatches) {
    use chrono::format::strftime::StrftimeItems;
    use libgitdit::Issue;

    use display::{FormattingToken as FT, MessageFmtToken as MFT, LineFormatter};
    use filters::MetadataFilter;

    let repo = util::open_dit_repo();
    let remote_prios = repo.remote_priorization();

    // construct filter
    let filter = match matches.values_of("filter") {
        Some(values) => {
            let specs = values.map(str::parse).abort_on_err();
            MetadataFilter::new(&remote_prios, specs).unwrap_or_abort()
        },
        None         => MetadataFilter::empty(&remote_prios),
    };

    let id_len = repo.abbreviation_length(matches);

    let formatter = if matches.is_present("long") {
        tokenvec![
            MFT::Id(id_len), FT::LineEnd,
            "Author: ", MFT::Author, FT::LineEnd,
            "Date: ", MFT::Date(StrftimeItems::new("%+")), FT::LineEnd,
            FT::LineEnd,
            MFT::Subject, FT::LineEnd,
            FT::LineEnd,
            MFT::BodyText,
            FT::LineEnd]
    } else {
        tokenvec![MFT::Id(id_len), " (", MFT::Date(StrftimeItems::new("%c")), ") ", MFT::Subject]
    };

    // get initial commits
    let mut issues : Vec<Issue> = repo
        .issues()
        .unwrap_or_abort()
        .into_iter()
        .filter(|issue| filter.filter(issue))
        .collect();

    // descending order
    let mut sort_key : Box<FnMut(&Issue) -> git2::Time> = Box::new(|ref issue| issue
        .initial_message()
        .unwrap_or_abort()
        .time());
    issues.sort_by(|a, b| sort_key(b).cmp(&sort_key(a)));

    // optionally limit to some number specified by the user
    if let Some(number) = matches.value_of("n") {
        // TODO: better error reporting?
        issues.truncate(str::parse(number).unwrap_or_abort());
    }

    // present the list to the user
    let result = issues
        .into_iter()
        .map(|issue| issue.initial_message())
        .abort_on_err()
        .flat_map(|initial| formatter.iter().formatted_lines(initial))
        .abort_on_err()
        .pipe_lines(repo.pager())
        .unwrap_or_abort();
    std::process::exit(result);
}


/// new subcommand implementation
///
fn mirror_impl(matches: &clap::ArgMatches) {
    use std::collections::HashSet;
    use gitext::{RemotePriorization, ReferrenceExt, ReferrencesExt};

    let repo = util::open_dit_repo();

    // retrieve the options and flags
    let remote = matches.value_of("remote");
    let clone_head = matches.is_present("clone-head");
    let update_head = matches.is_present("update-head");
    let create_leaves = matches.is_present("clone-leaves");

    let prios = remote
        .map(RemotePriorization::from)
        .unwrap_or_else(|| repo.remote_priorization());
    let issues = repo
        .cli_issues(matches)
        .unwrap_or_else(|| repo.issues().unwrap_or_abort());

    for issue in issues {
        if clone_head || update_head {
            // take care about the head reference
            if let Some(r) = issue.heads().abort_on_err().select_ref(&prios) {
                let id = r
                    .peel(git2::ObjectType::Commit)
                    .unwrap_or_abort()
                    .id();
                // TODO: Failure to update a head ref should probably result in
                //       a warning instead of a hard error.
                issue.update_head(id, update_head).unwrap_or_abort();
            }
        }

        if create_leaves {
            // construct a hash set with all the relevant leaves
            let mut leaves: HashSet<_> = issue
                .remote_refs(IssueRefType::Leaf)
                .abort_on_err()
                .filter(|reference| match remote {
                    Some(r) => reference
                        .remote()
                        .map(|name| name == r)
                        .unwrap_or(false),
                    None => true,
                })
                .map(|reference| reference
                    .peel(git2::ObjectType::Commit)
                    .unwrap_or_abort()
                    .id()
                )
                .collect();

            // Prepare revwalk for iterating over all messages which already
            // hang by local refs.
            let mut existing_refs = repo.revwalk().unwrap_or_abort();
            for parent in issue.initial_message().unwrap_or_abort().parent_ids() {
                existing_refs.hide(parent).unwrap_or_abort();
            }
            existing_refs.simplify_first_parent();
            existing_refs.set_sorting(git2::Sort::TOPOLOGICAL);
            for reference in issue.local_refs(IssueRefType::Any).abort_on_err() {
                existing_refs
                    .push(reference
                        .peel(git2::ObjectType::Commit)
                        .unwrap_or_abort()
                        .id()
                    )
                    .unwrap_or_abort();
            }
            {
                // This also includes future refs. We therefore add parents of
                // the supposed leaves as starting points of the revwalk. If any
                // of the ids cloned from the remote do not refer to leaves,
                // they will be filtered out early.
                let leaf_messages = leaves
                    .iter()
                    .cloned()
                    .map(|id| repo.find_commit(id))
                    .abort_on_err();
                for message in leaf_messages {
                    for parent in message.parent_ids() {
                        existing_refs.push(parent).unwrap_or_abort();
                    }
                }
            }

            // filter out any leaf which is not required
            for id in existing_refs.abort_on_err() {
                leaves.remove(&id);
            }

            // create refs for remaining leaves
            for leaf in leaves {
                issue.add_leaf(leaf).unwrap_or_abort();
            }
        }
    }
}


/// new subcommand implementation
///
fn new_impl(matches: &clap::ArgMatches) {
    use util::message_from_args;

    let repo = util::open_dit_repo();
    let author = repo.cli_author(matches);
    let committer = repo.signature().unwrap_or_abort();

    // get the message, either from the command line argument or an editor
    let message = if let Some(m) = message_from_args(matches) {
        // the message was supplied via the command line
        m.into_iter()
         .chain(repo.prepare_trailers(matches)
                    .into_iter()
                    .map(|t| t.to_string()))
         .collect()
    } else {
        // we need an editor

        // get the path where we want to edit the message
        let path = repo.commitmsg_edit_path(matches);

        { // write
            let mut file = File::create(path.as_path()).unwrap_or_abort();
            repo.prepare_trailers(matches)
                .write_lines(&mut file)
                .unwrap_or_abort();
            file.flush().unwrap_or_abort();
        }

        repo.get_commit_msg(path)
    }.into_iter().collect_string();

    // commit the message
    let tree = repo.empty_tree().unwrap_or_abort();
    let id = repo
        .create_issue(&author, &committer, message.trim(), &tree, Vec::new())
        .unwrap_or_abort();
    println!("[dit][new] {}", id);
}


/// push subcommand implementation
///
fn push_impl(matches: &clap::ArgMatches) {
    let repo = util::open_dit_repo();

    // note: "remote" is always present since it is a required parameter
    let mut remote = repo.find_remote(matches.value_of("remote").unwrap()).unwrap_or_abort();

    // accumulate the refspecs to push
    let refspecs : Vec<String> = repo
        .cli_issues(matches)
        .unwrap_or_else(|| repo.issues().unwrap_or_abort())
        .into_iter()
        .map(|issue| issue.local_refs(IssueRefType::Any))
        .abort_on_err()
        .flat_map(|mut refs| {
            let names: Vec<_> = refs
                .names()
                .abort_on_err()
                .map(String::from)
                .collect();
            names
        })
        .collect();

    // set the options for the push
    let mut fetch_options = git2::PushOptions::new();
    fetch_options.remote_callbacks(gitext::callbacks());

    let refspec_refs : Vec<&str> = refspecs.iter().map(String::as_str).collect();
    remote.push(refspec_refs.as_ref(), Some(&mut fetch_options))
          .unwrap_or_abort();
}


/// reply subcommand implementation
///
fn reply_impl(matches: &clap::ArgMatches) {
    use util::message_from_args;

    let repo = util::open_dit_repo();
    let author = repo.cli_author(matches);
    let committer = repo.signature().unwrap_or_abort();

    // NOTE: We want to do a lot of stuff early, because we want to report
    //       errors before a user spent time writing a commit message in her
    //       editor. This means that we have a lot of bindings which may not
    //       be neccessary otherwise, resulting in data lying around.

    // the unwrap is safe since `parent` is a required value
    // and get all the info from it that we might need
    let mut parent = repo.value_to_commit(matches.value_of("parent").unwrap());

    // extract the subject and tree from the parent
    let subject = parent.reply_subject();
    let tree = parent.tree().unwrap_or_abort();

    // figure out to what issue we reply
    let issue = repo.issue_with_message(&parent).unwrap_or_abort();

    // get the references specified on the command line
    let references = repo.cli_references(matches);

    // get the message, either from the command line argument or an editor
    let message = if let Some(m) = message_from_args(matches) {
        // the message was supplied via the command line
        if matches.is_present("quote") {
            warn!("Message will only quoted if an editor is used.");
        }

        m.into_iter()
         .chain(repo.prepare_trailers(matches)
                    .into_iter()
                    .map(|t| t.to_string()))
         .collect()
    } else {
        // we need an editor

        // get the path where we want to edit the message
        let path = repo.commitmsg_edit_path(matches);

        { // write
            let mut file = File::create(path.as_path()).unwrap_or_abort();
            if let Some(s) = subject {
                write!(&mut file, "{}\n\n", s).unwrap_or_abort();
            }

            if matches.is_present("quote") {
                parent
                    .body_lines()
                    .quoted()
                    .write_lines(&mut file)
                    .unwrap_or_abort();
                write!(&mut file, "\n").unwrap_or_abort();
            }

            repo.prepare_trailers(matches)
                .write_lines(&mut file)
                .unwrap_or_abort();
            file.flush().unwrap_or_abort();
        }

        repo.get_commit_msg(path)
    }.into_iter().collect_string();

    // construct a vector holding all parents
    let parent_refs = Some(&parent).into_iter().chain(references.iter());

    // finally, create the message
    issue.add_message(&author, &committer, message.trim(), &tree, parent_refs)
         .unwrap_or_abort();
}

/// show subcommand implementation
///
fn show_impl(matches: &clap::ArgMatches) {
    use chrono::format::strftime::StrftimeItems;

    use display::{FormattingToken as FT, MessageFmtToken as MFT, LineFormatter};
    use display::{IntoTreeGraph, TreeGraphElem, TreeGraphElemLine};
    use gitext::ReferrencesExt;

    let repo = util::open_dit_repo();
    let id_len = repo.abbreviation_length(matches);
    let prios = repo.remote_priorization();

    // NOTE: the issue is a required parameter
    let issue = repo.cli_issue(matches).unwrap();

    // translate commit to lines representing the commit
    let formatter : Vec<FT<_,_>> = if matches.is_present("msgtree") {
        // With the "tree" option, we only display subjects in a short
        // format
        tokenvec![MFT::Id(id_len), " ", MFT::Author, " ", MFT::Subject]
    } else {
        let head = issue
            .heads()
            .abort_on_err()
            .select_ref(&prios)
            .unwrap() // TODO: abort gracefully
            .target()
            .unwrap(); // TODO: abort gracefully

        tokenvec![
            MFT::Id(id_len), MFT::IfId(head, tokenvec![" (head)"]), FT::LineEnd,
            "Author: ", MFT::Author, FT::LineEnd,
            "Date: ", MFT::Date(StrftimeItems::new("%+")), FT::LineEnd,
            FT::LineEnd,
            MFT::Subject, FT::LineEnd,
            FT::LineEnd,
            MFT::Body,
            FT::LineEnd,
            FT::LineEnd]
    };

    // first, get us an iterator over all the commits
    let mut commits : Vec<(TreeGraphElemLine, Commit)> =
        if matches.is_present("initial") {
            vec![(
                TreeGraphElemLine::empty(),
                issue.initial_message().unwrap_or_abort()
            )]
        } else {
            issue
                .messages()
                .abort_on_err()
                .into_tree_graph()
                .collect()
        };

    // Decide on the order in which the messages will be printed.
    if matches.is_present("tree") {
        // We want the commits in chronological order
        commits.reverse();
        for commit in commits.iter_mut() {
            commit.0.reverse_marks();
        }
    };

    // Transform the simple graph element line into an iterator over lines to
    // print via multiple steps.
    let result = commits
        .into_iter()
        // expand the graph element lines for each message
        .map(|commit| {
            let mut elems = commit.0;
            // offset the commit from the graph elements by adding an empty one
            // in between
            elems.append(TreeGraphElem::Empty);
            (elems.commit_iterator(), commit.1)
        })
        // expand the message to a series of lines
        .flat_map(|commit| commit
            .0
            .zip(formatter.iter().formatted_lines(commit.1).abort_on_err())
        )
        // combine each line of graph elements and message
        .map(|line| format!("{} {}", line.0, line.1))
        .pipe_lines(repo.pager())
        .unwrap_or_abort();

    std::process::exit(result);
}

/// tag subcommand implementation
///
fn tag_impl(matches: &clap::ArgMatches) {
    use libgitdit::trailer::Trailer;
    use std::str::FromStr;

    use gitext::ReferrencesExt;

    let repo = util::open_dit_repo();
    let author = repo.cli_author(matches);
    let committer = repo.signature().unwrap_or_abort();
    let prios = repo.remote_priorization();

    // get the head for the issue to tag

    // NOTE: the issue is a required parameter
    let issue = repo.cli_issue(matches).unwrap();
    let mut head_commit = issue
        .heads()
        .abort_on_err()
        .select_ref(&prios)
        .unwrap() // TODO: abort gracefully
        .peel(git2::ObjectType::Commit)
        .unwrap_or_abort()
        .into_commit()
        .ok()
        .unwrap();

    if matches.is_present("list") {
        // we only list the metadata
        repo.issue_messages_iter(head_commit)
            .abort_on_err()
            .flat_map(|c| c.trailers())
            .print_lines()
            .unwrap_or_abort();
        return;
    }

    // we produce a commit with status and references

    // get references and trailers for the new commit
    let references = repo.cli_references(matches);
    let trailers : Vec<Trailer> = matches.values_of("set-status")
                                         .into_iter()
                                         .flat_map(|values| values)
                                         .map(Trailer::from_str)
                                         .abort_on_err()
                                         .collect();
    if references.is_empty() && trailers.is_empty() {
        warn!("No commit was created because no reference or tags were supplied.");
        return;
    }

    // construct the message
    let message = [head_commit.reply_subject().unwrap_or_default(), String::new()]
        .to_vec()
        .into_iter()
        .chain(trailers.into_iter().map(|t| t.to_string()))
        .collect_string();
    let tree = repo.empty_tree().unwrap_or_abort();
    let parent_refs : Vec<&Commit> = Some(&head_commit).into_iter().chain(references.iter()).collect();
    let new = repo
        .commit(None, &author, &committer, message.trim(), &tree, &parent_refs)
        .unwrap_or_abort();

    // update the head reference
    issue.update_head(new, true).unwrap_or_abort();
}


// Unknown subcommand handler

/// Handle unknown subcommands
///
/// Try to invoke an executable matching the name of the subcommand.
///
fn handle_unknown_subcommand(name: &str, matches: &clap::ArgMatches) {
    use std::process::Command;

    // prepare the command to be invoked
    let mut command = Command::new(format!("git-dit-{}", name));
    if let Some(values) = matches.values_of("") {
         values.fold(&mut command, |c, arg| c.arg(arg));
    }

    // run the command
    let result = command
        .spawn()
        .and_then(|mut child| child.wait())
        .unwrap_or_abort();
    if !result.success() {
        std::process::exit(result.code().unwrap_or(1));
    }
}


fn main() {
    let yaml    = load_yaml!("cli.yaml");
    let matches = App::from_yaml(yaml).get_matches();

    if let Err(err) = system::Logger::init(Level::Warn) {
        writeln!(io::stderr(), "Could not initialize logger: {}", err).ok();
    }

    match matches.subcommand() {
        // Plumbing subcommands
        ("check-message",               Some(sub_matches)) => check_message(sub_matches),
        ("check-refname",               Some(sub_matches)) => check_refname(sub_matches),
        ("create-message",              Some(sub_matches)) => create_message(sub_matches),
        ("find-tree-init-hash",         Some(sub_matches)) => find_tree_init_hash(sub_matches),
        ("get-issue-metadata",          Some(sub_matches)) => get_issue_metadata(sub_matches),
        ("get-issue-tree-init-hashes",  Some(sub_matches)) => get_issue_tree_init_hashes(sub_matches),
        // Porcelain subcommands
        ("fetch",   Some(sub_matches)) => fetch_impl(sub_matches),
        ("gc",      Some(sub_matches)) => gc_impl(sub_matches),
        ("list",    Some(sub_matches)) => list_impl(sub_matches),
        ("mirror",  Some(sub_matches)) => mirror_impl(sub_matches),
        ("new",     Some(sub_matches)) => new_impl(sub_matches),
        ("push",    Some(sub_matches)) => push_impl(sub_matches),
        ("reply",   Some(sub_matches)) => reply_impl(sub_matches),
        ("show",    Some(sub_matches)) => show_impl(sub_matches),
        ("tag",     Some(sub_matches)) => tag_impl(sub_matches),
        // Unknown subcommands
        ("", _) => {
            writeln!(io::stderr(), "{}", matches.usage()).ok();
            std::process::exit(1);
        },
        (name, sub_matches) => {
            let default = clap::ArgMatches::default();
            handle_unknown_subcommand(name, sub_matches.unwrap_or(&default))
        },
    }
}
