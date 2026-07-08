use axonmind_engine::structure::{derive_identity, parse_document, StructurePackage};

#[test]
fn repro_gdpr_regulation_zero_units() {
    let pkg_dir = std::path::Path::new(
        "/Users/xuejingzhoum3/GitHub/soverex/soverex-open/docs/legal_agent/structure",
    );
    let pkg = StructurePackage::from_dir(pkg_dir).expect("load package");

    let markdown = std::fs::read_to_string(
        "/private/tmp/claude-501/-Users-xuejingzhoum3-GitHub-soverex-soverex-open/3a617ab6-8107-452c-86e6-bb91ddbfa743/scratchpad/gdpr.md",
    )
    .expect("read markdown");

    let derived = derive_identity(
        "doc.b515d17b",
        Some("legal_gdpr_Regulation_2016_679.pdf"),
        Some("/x/legal_gdpr_Regulation_2016_679.pdf"),
        Some(&markdown),
        std::slice::from_ref(&pkg),
    );
    eprintln!("profile_name = {:?}", derived.profile_name);
    eprintln!("instrument_type = {:?}", derived.identity.instrument_type);
    eprintln!("corpus = {:?}", derived.identity.corpus);

    let profile_name = derived.profile_name.clone().expect("profile matched");
    let profile = pkg
        .profiles
        .iter()
        .find(|p| p.profile.name == profile_name)
        .expect("profile found in package");

    let result = parse_document(
        &pkg.manifest.package.name,
        profile,
        "doc.b515d17b",
        &derived.identity,
        &markdown,
    );
    match &result {
        Ok(Some(parsed)) => eprintln!("units = {}", parsed.units.len()),
        Ok(None) => eprintln!("parse_document returned Ok(None) -- zero markers found"),
        Err(e) => eprintln!("parse_document ERROR: {e}"),
    }
    assert!(result.is_ok());
}
