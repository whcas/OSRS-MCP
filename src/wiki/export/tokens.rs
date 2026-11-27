//! Turns the XML parser's events into the small set of tokens the importer
//! needs, enforcing the nesting limit and mapping every failure to an
//! [`ExportError`] that carries no document text.

use std::io::BufRead;

use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::ResolveResult;
use quick_xml::reader::NsReader;

use super::error::{ExportError, LimitKind, XmlFault};
use super::input::BoundedInput;

/// The `MediaWiki` export namespace this importer supports.
pub(super) const EXPORT_NAMESPACE: &str = "http://www.mediawiki.org/xml/export-0.11/";

/// A start tag, reduced to what the importer reads.
#[derive(Debug)]
pub(super) struct Element {
    name: String,
    in_export_namespace: bool,
    attributes: Vec<(String, String)>,
}

impl Element {
    /// The local name, if the element is in the export namespace. Elements in
    /// other namespaces (extensions) have no export name and are skipped.
    pub(super) fn export_name(&self) -> Option<&str> {
        self.in_export_namespace.then_some(self.name.as_str())
    }

    /// The local name regardless of namespace.
    pub(super) fn local_name(&self) -> &str {
        &self.name
    }

    pub(super) const fn is_in_export_namespace(&self) -> bool {
        self.in_export_namespace
    }

    /// The unescaped value of the attribute with this exact name.
    pub(super) fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug)]
pub(super) enum Token {
    /// An element opened. Empty elements produce a start and an end.
    Start(Element),
    /// The innermost open element closed.
    End,
    /// Character data, CDATA, or one resolved character or entity reference.
    /// Line endings are normalised as XML requires.
    Text(String),
    /// The input ended with every element closed.
    Eof,
}

/// The content of an element that should hold only text.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum FieldContent {
    Text(String),
    /// More text than the caller allowed; none of it is kept.
    TooLong,
    /// The element contained child elements.
    Nested,
}

/// Why an event could not be translated. Resolved to an [`ExportError`] with a
/// position by [`TokenReader::explain`].
enum Failure {
    Xml(quick_xml::Error),
    Doctype,
    Encoding,
    Reference,
    Attribute,
    Syntax,
}

pub(super) struct TokenReader<R> {
    reader: NsReader<BoundedInput<R>>,
    buffer: Vec<u8>,
    depth: usize,
    max_depth: usize,
}

impl<R: BufRead> TokenReader<R> {
    pub(super) fn new(input: R, max_input_bytes: u64, max_depth: usize) -> Self {
        let mut reader = NsReader::from_reader(BoundedInput::new(input, max_input_bytes));
        // Empty elements such as `<text />` then look like a start and an end.
        reader.config_mut().expand_empty_elements = true;
        Self {
            reader,
            buffer: Vec::new(),
            depth: 0,
            max_depth,
        }
    }

    /// Begins counting a page against its own size limit.
    pub(super) fn begin_page(&mut self, max_bytes: u64) {
        self.reader.get_mut().begin_page(max_bytes);
    }

    pub(super) fn end_page(&mut self) {
        self.reader.get_mut().end_page();
    }

    /// Reads the next token.
    ///
    /// # Errors
    ///
    /// Returns an [`ExportError`] for malformed, truncated, over-limit or
    /// unreadable input, and for anything the importer refuses to process.
    pub(super) fn next_token(&mut self) -> Result<Token, ExportError> {
        loop {
            self.buffer.clear();
            let translated = match self.reader.read_resolved_event_into(&mut self.buffer) {
                Ok((namespace, event)) => translate(&namespace, event),
                Err(error) => Err(Failure::Xml(error)),
            };
            match translated.map_err(|failure| self.explain(failure))? {
                Translated::Skip => {}
                Translated::Token(token) => return self.track_depth(token),
            }
        }
    }

    /// Reads and discards everything up to and including the end of the
    /// element whose start tag was just read.
    pub(super) fn skip_element(&mut self) -> Result<(), ExportError> {
        let outer = self.depth.saturating_sub(1);
        while self.depth > outer {
            self.next_token()?;
        }
        Ok(())
    }

    /// Reads the content of the element whose start tag was just read, up to
    /// and including its end tag. At most `max_bytes` of text are kept.
    pub(super) fn read_field(&mut self, max_bytes: usize) -> Result<FieldContent, ExportError> {
        let mut text = String::new();
        let mut too_long = false;
        let mut nested = false;
        loop {
            match self.next_token()? {
                Token::Text(chunk) => {
                    if too_long || text.len().saturating_add(chunk.len()) > max_bytes {
                        too_long = true;
                        text = String::new();
                    } else {
                        text.push_str(&chunk);
                    }
                }
                Token::Start(_) => {
                    nested = true;
                    self.skip_element()?;
                }
                Token::End => break,
                Token::Eof => return Err(self.truncated()),
            }
        }
        Ok(if nested {
            FieldContent::Nested
        } else if too_long {
            FieldContent::TooLong
        } else {
            FieldContent::Text(text)
        })
    }

    /// The error for a document that is well-formed so far but not shaped like
    /// an export, located at the current position.
    pub(super) fn structure_error(&self) -> ExportError {
        ExportError::Malformed {
            fault: XmlFault::Structure,
            offset: self.reader.buffer_position(),
        }
    }

    fn truncated(&self) -> ExportError {
        ExportError::Truncated {
            offset: self.reader.buffer_position(),
        }
    }

    fn track_depth(&mut self, token: Token) -> Result<Token, ExportError> {
        match &token {
            Token::Start(_) => {
                self.depth += 1;
                if self.depth > self.max_depth {
                    return Err(ExportError::LimitExceeded {
                        limit: LimitKind::Depth,
                        max: u64::try_from(self.max_depth).unwrap_or(u64::MAX),
                    });
                }
            }
            Token::End => self.depth = self.depth.saturating_sub(1),
            Token::Eof if self.depth > 0 => return Err(self.truncated()),
            Token::Text(_) | Token::Eof => {}
        }
        Ok(token)
    }

    fn explain(&self, failure: Failure) -> ExportError {
        let offset = self.reader.buffer_position();
        let malformed = |fault| ExportError::Malformed { fault, offset };
        match failure {
            Failure::Doctype => ExportError::DoctypeNotAllowed { offset },
            Failure::Encoding => ExportError::UnsupportedEncoding,
            Failure::Reference => malformed(XmlFault::Reference),
            Failure::Attribute => malformed(XmlFault::Attribute),
            Failure::Syntax => malformed(XmlFault::Syntax),
            Failure::Xml(error) => self.explain_xml(&error, offset),
        }
    }

    fn explain_xml(&self, error: &quick_xml::Error, offset: u64) -> ExportError {
        use quick_xml::Error;
        use quick_xml::errors::{IllFormedError, SyntaxError};

        let malformed = |fault| ExportError::Malformed { fault, offset };
        match error {
            Error::Io(io_error) => match self.reader.get_ref().exceeded() {
                Some((limit, max)) => ExportError::LimitExceeded { limit, max },
                None => ExportError::Read {
                    kind: io_error.kind(),
                },
            },
            Error::Syntax(SyntaxError::InvalidBangMarkup) => malformed(XmlFault::Syntax),
            // Every other syntax error means the input ended inside markup.
            Error::Syntax(_) => ExportError::Truncated { offset },
            Error::IllFormed(
                IllFormedError::MismatchedEndTag { .. }
                | IllFormedError::UnmatchedEndTag(_)
                | IllFormedError::MissingEndTag(_),
            ) => malformed(XmlFault::TagMismatch),
            Error::IllFormed(IllFormedError::UnclosedReference) | Error::Escape(_) => {
                malformed(XmlFault::Reference)
            }
            Error::InvalidAttr(_) => malformed(XmlFault::Attribute),
            Error::Encoding(_) => malformed(XmlFault::Encoding),
            Error::Namespace(_) => malformed(XmlFault::Namespace),
            Error::IllFormed(_) => malformed(XmlFault::Syntax),
        }
    }
}

enum Translated {
    Token(Token),
    /// Comments, processing instructions and the XML declaration.
    Skip,
}

fn translate(namespace: &ResolveResult<'_>, event: Event<'_>) -> Result<Translated, Failure> {
    let token = match event {
        Event::Start(start) => Token::Start(element(namespace, &start)?),
        Event::End(_) => Token::End,
        Event::Text(text) => Token::Text(text.xml10_content().into_owned()),
        Event::CData(data) => Token::Text(data.xml10_content().into_owned()),
        Event::GeneralRef(reference) => Token::Text(resolve_reference(&reference)?),
        Event::Decl(declaration) => return check_encoding(&declaration),
        Event::DocType(_) => return Err(Failure::Doctype),
        Event::Comment(_) | Event::PI(_) => return Ok(Translated::Skip),
        // `expand_empty_elements` is set, so this indicates a parser contract
        // change rather than bad input.
        Event::Empty(_) => return Err(Failure::Syntax),
        Event::Eof => Token::Eof,
    };
    Ok(Translated::Token(token))
}

fn element(namespace: &ResolveResult<'_>, start: &BytesStart<'_>) -> Result<Element, Failure> {
    let attributes = start
        .attributes()
        .map(|attribute| {
            let attribute = attribute.map_err(|_| Failure::Attribute)?;
            let value = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|_| Failure::Reference)?
                .into_owned();
            Ok((attribute.key.as_ref().to_owned(), value))
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    Ok(Element {
        name: start.local_name().as_ref().to_owned(),
        in_export_namespace: matches!(
            namespace,
            ResolveResult::Bound(bound) if bound.0 == EXPORT_NAMESPACE
        ),
        attributes,
    })
}

/// Resolves a character reference or one of the five predefined entities.
/// Any other entity would need a declaration, which is never processed.
fn resolve_reference(reference: &quick_xml::events::BytesRef<'_>) -> Result<String, Failure> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|_| Failure::Reference)?
    {
        return Ok(character.to_string());
    }
    resolve_predefined_entity(&reference.xml10_content())
        .map(str::to_owned)
        .ok_or(Failure::Reference)
}

fn check_encoding(declaration: &quick_xml::events::BytesDecl<'_>) -> Result<Translated, Failure> {
    match declaration.encoding() {
        None => Ok(Translated::Skip),
        Some(Ok(encoding)) if encoding.eq_ignore_ascii_case("utf-8") => Ok(Translated::Skip),
        Some(_) => Err(Failure::Encoding),
    }
}
