//! Rule ids and findings. Ids are stable: a rule keeps its id forever, a retired id is never
//! reused, and the README documents each one (Rule, Why, Enforced by).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
    FileLength,
    LengthAllowlist,
    RoleSuffix,
    VagueModule,
    CrateDirection,
    UiDependency,
    UnsafeOutsidePlatform,
    FolderBudget,
    DenyWarnings,
    MemberProfile,
    WorkspaceDependency,
    TodoComment,
    BlockOn,
    Expectation,
    TestSupportFeature,
    MotionInstall,
}

impl Rule {
    pub const ALL: [Rule; 16] = [
        Rule::FileLength,
        Rule::LengthAllowlist,
        Rule::RoleSuffix,
        Rule::VagueModule,
        Rule::CrateDirection,
        Rule::UiDependency,
        Rule::UnsafeOutsidePlatform,
        Rule::FolderBudget,
        Rule::DenyWarnings,
        Rule::MemberProfile,
        Rule::WorkspaceDependency,
        Rule::TodoComment,
        Rule::BlockOn,
        Rule::Expectation,
        Rule::TestSupportFeature,
        Rule::MotionInstall,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Rule::FileLength => "RL001",
            Rule::LengthAllowlist => "RL002",
            Rule::RoleSuffix => "RL003",
            Rule::VagueModule => "RL004",
            Rule::CrateDirection => "RL005",
            Rule::UiDependency => "RL006",
            Rule::UnsafeOutsidePlatform => "RL007",
            Rule::FolderBudget => "RL008",
            Rule::DenyWarnings => "RL009",
            Rule::MemberProfile => "RL010",
            Rule::WorkspaceDependency => "RL011",
            Rule::TodoComment => "RL012",
            Rule::BlockOn => "RL013",
            Rule::Expectation => "RL014",
            Rule::TestSupportFeature => "RL015",
            Rule::MotionInstall => "RL016",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Rule::FileLength => "file-length",
            Rule::LengthAllowlist => "length-allowlist",
            Rule::RoleSuffix => "role-suffix",
            Rule::VagueModule => "vague-module",
            Rule::CrateDirection => "crate-direction",
            Rule::UiDependency => "ui-dependency",
            Rule::UnsafeOutsidePlatform => "unsafe-outside-platform",
            Rule::FolderBudget => "folder-budget",
            Rule::DenyWarnings => "deny-warnings",
            Rule::MemberProfile => "member-profile",
            Rule::WorkspaceDependency => "workspace-dependency",
            Rule::TodoComment => "todo-comment",
            Rule::BlockOn => "block-on",
            Rule::Expectation => "expectation",
            Rule::TestSupportFeature => "test-support-feature",
            Rule::MotionInstall => "motion-install",
        }
    }

    /// Rules that a `// repo-lint: expect(RLxxx) <reason>` comment may waive at one line.
    /// The others have their own registry (allowlist, `.folder-budget`, the direction table).
    pub fn is_waivable(self) -> bool {
        matches!(
            self,
            Rule::RoleSuffix
                | Rule::VagueModule
                | Rule::UnsafeOutsidePlatform
                | Rule::DenyWarnings
                | Rule::TodoComment
                | Rule::BlockOn
        )
    }

    pub fn from_id(id: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|rule| rule.id() == id)
    }
}

/// One finding: where and what, with the fix in the message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub path: String,
    pub line: Option<usize>,
    pub rule: Rule,
    pub message: String,
}

impl Finding {
    pub fn file(rule: Rule, path: &str, message: String) -> Finding {
        Finding { path: path.to_owned(), line: None, rule, message }
    }

    pub fn at(rule: Rule, path: &str, line: usize, message: String) -> Finding {
        Finding { path: path.to_owned(), line: Some(line), rule, message }
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "{}:{line}: ", self.path)?,
            None => write!(f, "{}: ", self.path)?,
        }
        write!(f, "{} {}: {}", self.rule.id(), self.rule.name(), self.message)
    }
}
