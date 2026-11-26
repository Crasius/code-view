//! A pull request page out of sight hands its hibernation down to its Files tab, which drops what
//! loads again and keeps what the user did (`cv_pr_files`, `tab/hibernate.rs`).
//!
//! The impl lives in the page's crate because both the trait (`cv-runtime`) and the type are
//! foreign anywhere else, and in a child of the crate root because it reads the private `files`.

use cv_runtime::cache::memory::Hibernate;

use crate::PullRequestPage;

impl Hibernate for PullRequestPage {
    fn hibernate(&mut self) {
        self.files.hibernate();
    }
}
