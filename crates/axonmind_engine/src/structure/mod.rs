pub mod evals;
pub mod identity;
pub mod model;
pub mod parse;
pub mod registry;

pub use evals::{EvalCaseResult, EvalOutcome, PackageEvalReport, eval_case_result};
pub use identity::{DerivedIdentity, derive_identity, identity_matches};
pub use model::{
    CaptureSelector, CorpusBinding, CorpusFile, EvalCase, EvalsFile, IdentityMatch, IdentityRule,
    IdentityRulesFile, PackageManifest, ProfileDefinition, StructurePackage, StructureUnit,
    StructureUnitRef, XrefBinding,
};
pub use parse::{ParsedDocumentIndex, ParsedUnit, parse_document};
pub use registry::{InstallReport, PackageInfo};
