//! Reads the export's root element and `siteinfo` header.

use std::io::BufRead;

use super::error::ExportError;
use super::tokens::{Element, FieldContent, Token, TokenReader};
use crate::wiki::{SourceField, SourceId};

const SUPPORTED_VERSION: &str = "0.11";
/// Site names, database names and URLs are short; anything longer is invalid
/// and is not worth buffering.
const MAX_HEADER_FIELD_BYTES: usize = 4096;

/// Reads up to the end of `siteinfo` and returns the site's identity.
///
/// # Errors
///
/// Returns [`ExportError::NotAnExport`] if the root element is not
/// `mediawiki`, [`ExportError::UnsupportedSchema`] if it is not export schema
/// 0.11, [`ExportError::MissingSiteInfo`] if a page or the end of the document
/// comes before `siteinfo`, and [`ExportError::InvalidSiteInfo`] if a required
/// field is missing, repeated or invalid.
pub(super) fn read_header<R: BufRead>(
    tokens: &mut TokenReader<R>,
) -> Result<SourceId, ExportError> {
    read_root(tokens)?;
    read_siteinfo(tokens)
}

fn read_root<R: BufRead>(tokens: &mut TokenReader<R>) -> Result<(), ExportError> {
    loop {
        match tokens.next_token()? {
            Token::Start(root) => return check_root(&root),
            Token::Text(text) if text.trim().is_empty() => {}
            Token::Text(_) | Token::End | Token::Eof => return Err(ExportError::NotAnExport),
        }
    }
}

fn check_root(root: &Element) -> Result<(), ExportError> {
    if root.local_name() != "mediawiki" {
        return Err(ExportError::NotAnExport);
    }
    if root.is_in_export_namespace() && root.attribute("version") == Some(SUPPORTED_VERSION) {
        Ok(())
    } else {
        Err(ExportError::UnsupportedSchema)
    }
}

fn read_siteinfo<R: BufRead>(tokens: &mut TokenReader<R>) -> Result<SourceId, ExportError> {
    loop {
        match tokens.next_token()? {
            Token::Start(element) if element.export_name() == Some("siteinfo") => {
                return read_siteinfo_fields(tokens);
            }
            Token::Start(element) if element.export_name() == Some("page") => {
                return Err(ExportError::MissingSiteInfo);
            }
            Token::Start(_) => tokens.skip_element()?,
            Token::Text(text) if text.trim().is_empty() => {}
            Token::Text(_) => return Err(tokens.structure_error()),
            Token::End | Token::Eof => return Err(ExportError::MissingSiteInfo),
        }
    }
}

#[derive(Default)]
struct SiteFields {
    site_name: Option<FieldContent>,
    db_name: Option<FieldContent>,
    base_url: Option<FieldContent>,
}

fn read_siteinfo_fields<R: BufRead>(tokens: &mut TokenReader<R>) -> Result<SourceId, ExportError> {
    let mut fields = SiteFields::default();
    loop {
        match tokens.next_token()? {
            Token::Start(element) => {
                let slot = match element.export_name() {
                    Some("sitename") => Some((&mut fields.site_name, SourceField::SiteName)),
                    Some("dbname") => Some((&mut fields.db_name, SourceField::DbName)),
                    Some("base") => Some((&mut fields.base_url, SourceField::BaseUrl)),
                    _ => None,
                };
                match slot {
                    Some((slot, field)) => read_once(tokens, slot, field)?,
                    None => tokens.skip_element()?,
                }
            }
            Token::Text(_) => {}
            Token::End => return fields.into_source(),
            Token::Eof => return Err(tokens.structure_error()),
        }
    }
}

/// Reads a field into `slot`, refusing a second occurrence.
fn read_once<R: BufRead>(
    tokens: &mut TokenReader<R>,
    slot: &mut Option<FieldContent>,
    field: SourceField,
) -> Result<(), ExportError> {
    if slot.is_some() {
        return Err(ExportError::InvalidSiteInfo { field });
    }
    *slot = Some(tokens.read_field(MAX_HEADER_FIELD_BYTES)?);
    Ok(())
}

impl SiteFields {
    fn into_source(self) -> Result<SourceId, ExportError> {
        let site_name = required(self.site_name, SourceField::SiteName)?;
        let db_name = optional(self.db_name, SourceField::DbName)?;
        let base_url = required(self.base_url, SourceField::BaseUrl)?;
        SourceId::new(site_name, db_name, base_url)
            .map_err(|field| ExportError::InvalidSiteInfo { field })
    }
}

fn required(content: Option<FieldContent>, field: SourceField) -> Result<String, ExportError> {
    optional(content, field)?.ok_or(ExportError::InvalidSiteInfo { field })
}

fn optional(
    content: Option<FieldContent>,
    field: SourceField,
) -> Result<Option<String>, ExportError> {
    match content {
        None => Ok(None),
        Some(FieldContent::Text(text)) => Ok(Some(text)),
        Some(FieldContent::TooLong | FieldContent::Nested) => {
            Err(ExportError::InvalidSiteInfo { field })
        }
    }
}
