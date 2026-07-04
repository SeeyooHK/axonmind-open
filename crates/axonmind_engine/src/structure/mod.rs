pub mod identity;
pub mod model;
pub mod parse;
pub mod registry;

pub use identity::{DerivedIdentity, derive_identity};
pub use model::{
    CaptureSelector, CorpusBinding, CorpusFile, IdentityRule, IdentityRulesFile, PackageManifest,
    ProfileDefinition, StructurePackage, StructureUnit, StructureUnitRef,
};
pub use parse::{ParsedDocumentIndex, ParsedUnit, parse_document};
pub use registry::{InstallReport, PackageInfo};
