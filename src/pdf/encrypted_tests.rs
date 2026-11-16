//! Encrypted PDFs (issue #36) must be refused with a clear error,
//! including owner-password-only files whose user password is empty.

use super::tests::fixture_pdf;
use super::*;
use crate::test_util::TempFile;

/// Save the one-page fixture encrypted with the given passwords.
fn write_encrypted_pdf(name: &str, owner_password: &str, user_password: &str) -> TempFile {
    use lopdf::{EncryptionState, EncryptionVersion, Permissions};
    let (mut fixture, _out) = fixture_pdf();
    // Encryption keys are derived from the trailer file ID.
    fixture.doc.trailer.set(
        "ID",
        vec![
            Object::String(vec![0x54; 16], StringFormat::Hexadecimal),
            Object::String(vec![0x54; 16], StringFormat::Hexadecimal),
        ],
    );
    let state = EncryptionState::try_from(EncryptionVersion::V2 {
        document: &fixture.doc,
        owner_password,
        user_password,
        key_length: 128,
        permissions: Permissions::default(),
    })
    .unwrap();
    fixture.doc.encrypt(&state).unwrap();
    let file = TempFile::new(name);
    fixture.doc.save(file.path()).unwrap();
    file
}

#[test]
fn open_owner_password_only_pdf_is_refused() {
    // Empty user password: lopdf/hayro silently decrypt these, so
    // without an explicit check the file would open (and a save
    // would strip the owner-password restrictions).
    let input = write_encrypted_pdf("owner-only", "owner-secret", "");
    let Err(err) = PdfDoc::open(input.path()) else {
        panic!("owner-password-only PDF must not open");
    };
    assert!(
        format!("{err:#}").contains("Encrypted/password-protected PDFs are not supported"),
        "unexpected: {err:#}"
    );
}

#[test]
fn open_user_password_pdf_is_refused() {
    let input = write_encrypted_pdf("user-password", "owner-secret", "user-secret");
    let Err(err) = PdfDoc::open(input.path()) else {
        panic!("user-password PDF must not open");
    };
    assert!(
        format!("{err:#}").contains("Encrypted/password-protected PDFs are not supported"),
        "unexpected: {err:#}"
    );
}
