// git-dit - the distributed issue tracker for git
// Copyright (C) 2016, 2017 Matthias Beyer <mail@beyermatthias.de>
// Copyright (C) 2016, 2017 Julian Ganz <neither@nut.email>
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.
//

//! Garbage collecting utilities
//!
//! This module provides git-dit related garbage collection utilites.
//!

use git2::{self, Reference};

use issue::{Issue, IssueRefType};
use iter::{self, RefsReferringTo};
use utils::ResultIterExt;

use error::*;
use error::Kind as EK;


/// Reference collecting iterator
///
/// This is a convenience type for a `ReferenceDeletingIter` wrapping an
/// iterator over to-be-collected references.
///
pub type ReferenceCollector<'r> = iter::ReferenceDeletingIter<
    'r,
    <Vec<Reference<'r>> as IntoIterator>::IntoIter
>;


pub enum ReferenceCollectionSpec {
    Never,
    BackedByRemoteHead,
}


/// Type representing collectable references
///
/// Use this type in order to compute dit-references which are no longer
/// required and thus may be collected.
///
pub struct CollectableRefs<'r>
{
    repo: &'r git2::Repository,
    /// Should remote references be considered during collection?
    consider_remote_refs: bool,
    /// Under what circumstances should local heads be collected?
    collect_heads: ReferenceCollectionSpec,
}

impl<'r> CollectableRefs<'r>
{
    /// Create a new CollectableRefs object
    ///
    /// By default only local references are considered, e.g. references which
    /// are unnecessary due to remote references are not reported.
    ///
    pub fn new(repo: &'r git2::Repository) -> Self
    {
        CollectableRefs {
            repo: repo,
            consider_remote_refs: false,
            collect_heads: ReferenceCollectionSpec::Never,
        }
    }

    /// Causes remote references to be considered
    ///
    /// By default, only local references are considered for deciding which
    /// references will be collected. Calling this function causes the resulting
    /// struct to also consider remote references.
    ///
    pub fn consider_remote_refs(mut self, option: bool) -> Self {
        self.consider_remote_refs = option;
        self
    }

    /// Causes local head references to be collected under a specified condition
    ///
    /// By default, heads are never collected. Using this function a user may
    /// change this behaviour.
    ///
    pub fn collect_heads(mut self, condition: ReferenceCollectionSpec) -> Self {
        self.collect_heads = condition;
        self
    }

    /// Find collectable references for an issue
    ///
    /// Construct an iterator yielding all collectable references for a given
    /// issue, according to the configuration.
    ///
    pub fn for_issue(&self, issue: &Issue<'r>) -> Result<RefsReferringTo<'r>, git2::Error> {
        let mut retval = {
            let messages = self
                .repo
                .revwalk()
                .chain_err(|| EK::CannotConstructRevwalk)?;
            RefsReferringTo::new(messages)
        };

        // local head
        if let Some(local_head) = issue.local_head().ok() {
            // Its ok to ignore failures to retrieve the local head. It will
            // not be present in user's repositories anyway.
            retval.push(
                local_head
                    .peel(git2::ObjectType::Commit)
                    .chain_err(|| EK::CannotGetCommit)?
                    .id()
            )?;

            // Whether the local head should be collected or not is computed
            // here, in the exact same way it is for leaves. We do that
            // because can't mix the computation with those of the leaves.
            // It would cause head references to be removed if any message
            // was posted as a reply to the current head.
            let mut head_history = self
                .repo
                .revwalk()
                .chain_err(|| EK::CannotConstructRevwalk)?;
            match self.collect_heads {
                ReferenceCollectionSpec::Never => {},
                ReferenceCollectionSpec::BackedByRemoteHead => {
                    for item in issue.remote_refs(IssueRefType::Head)? {
                        head_history.push(
                            item?
                                .peel(git2::ObjectType::Commit)
                                .chain_err(|| EK::CannotGetCommit)?
                                .id()
                        )?;
                    }
                },
            };
            let mut referring_refs = iter::RefsReferringTo::new(head_history);
            referring_refs.watch_ref(local_head)?;
            referring_refs.collect_result_into(&mut retval)?;
        }

        // local leaves
        for item in issue.local_refs(IssueRefType::Leaf)? {
            let leaf = item?;
            // NOTE: We push the parents of the references rather than the
            //       references themselves since that would cause the
            //       `RefsReferringTo` report that exact same reference.
            Self::push_ref_parents(&mut retval, &leaf)?;
            retval.watch_ref(leaf)?;
        }

        // remote refs
        if self.consider_remote_refs {
            for item in issue.remote_refs(IssueRefType::Any)? {
                retval.push(item?
                    .peel(git2::ObjectType::Commit)
                    .chain_err(|| EK::CannotGetCommit)?
                    .id()
                )?;
            }
        }

        Ok(retval)
    }

    /// Push the parents of a referred commit to a revwalk
    ///
    fn push_ref_parents<'a>(
        target: &mut RefsReferringTo,
        reference: &'a Reference<'a>,
    ) -> Result<(), git2::Error> {
        let referred_commit = reference
            .peel(git2::ObjectType::Commit)
            .chain_err(|| EK::CannotGetCommit)?
            .into_commit()
            .map_err(|o| Error::from_kind(EK::CannotGetCommitForRev(o.id().to_string())))?;
        for parent in referred_commit.parent_ids() {
            target.push(parent)?;
        }
        Ok(())
    }
}




#[cfg(test)]
mod tests {
    use super::*;
    use test_utils::TestingRepo;

    use repository::RepositoryExt;

    // CollectableRefs tests

    #[test]
    fn collectable_leaves() {
        let mut testing_repo = TestingRepo::new("collectable_leaves");
        let repo = testing_repo.repo();

        let sig = git2::Signature::now("Foo Bar", "foo.bar@example.com")
            .expect("Could not create signature");
        let empty_tree = repo
            .empty_tree()
            .expect("Could not create empty tree");

        let mut refs_to_collect = Vec::new();
        let mut issues = Vec::new();

        {
            // issue not supposed to be affected
            let issue = repo
                .create_issue(&sig, &sig, "Test message 1", &empty_tree, vec![])
                .expect("Could not create issue");
            let initial_message = issue
                .initial_message()
                .expect("Could not retrieve initial message");
            issue.add_message(&sig, &sig, "Test message 2", &empty_tree, vec![&initial_message])
                .expect("Could not add message");
        }

        {
            let issue = repo
                .create_issue(&sig, &sig, "Test message 3", &empty_tree, vec![])
                .expect("Could not create issue");
            let initial_message = issue
                .initial_message()
                .expect("Could not retrieve initial message");
            let message = issue
                .add_message(&sig, &sig, "Test message 4", &empty_tree, vec![&initial_message])
                .expect("Could not add message");
            issue.update_head(message.id(), true).expect("Could not update head");
            issues.push(issue);
            refs_to_collect.push(message.id());
        }

        {
            let issue = repo
                .create_issue(&sig, &sig, "Test message 5", &empty_tree, vec![])
                .expect("Could not create issue");
            let initial_message = issue
                .initial_message()
                .expect("Could not retrieve initial message");
            let message1 = issue
                .add_message(&sig, &sig, "Test message 6", &empty_tree, vec![&initial_message])
                .expect("Could not add message");
            issue
                .add_message(&sig, &sig, "Test message 7", &empty_tree, vec![&message1])
                .expect("Could not add message");
            issues.push(issue);
            refs_to_collect.push(message1.id());
        }

        refs_to_collect.sort();

        let collectable = CollectableRefs::new(repo).collect_heads(ReferenceCollectionSpec::BackedByRemoteHead);
        let mut collected: Vec<_> = issues
            .iter()
            .flat_map(|i| collectable.for_issue(i).expect("Error during discovery of collectable refs"))
            .collect::<Result<Vec<_>, git2::Error>>()
            .expect("Error during collection")
            .into_iter()
            .map(|r| r.peel(git2::ObjectType::Commit).expect("Could not peel ref").id())
            .collect();
        collected.sort();
        assert_eq!(refs_to_collect, collected);
    }
}

