//! Bounded decoding of the JavaScript playground request.
//!
//! This target-specific imperative shell inspects only the fixed request
//! fields and array elements. It never recursively walks caller-owned values,
//! and it computes UTF-8 sizes from JavaScript's UTF-16 code units before any
//! string is copied into Wasm linear memory.

use js_sys::{JsString, Object, Reflect};
use thiserror::Error;
use wasm_bindgen::{JsCast, JsValue};

use crate::project::{
    MAX_PLAYGROUND_FILE_BYTES, MAX_PLAYGROUND_PATH_BYTES, PlaygroundFile, PlaygroundRequest,
    ProjectPathRole, ProjectValidationError, RequestSizeBudget, validate_file_count,
};

#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    /// Fallible binding for `Array.isArray`; the standard infallible binding
    /// can throw when passed a revoked JavaScript proxy.
    #[wasm_bindgen::prelude::wasm_bindgen(catch, js_namespace = Array, js_name = isArray)]
    fn try_is_array(value: &JsValue) -> Result<bool, JsValue>;
}

/// A JavaScript string proven to fit a fixed UTF-8 byte budget before it is
/// copied into Wasm linear memory.
struct BoundedJsString<const MAX_BYTES: usize> {
    value: JsString,
    utf8_bytes: usize,
}

impl<const MAX_BYTES: usize> BoundedJsString<MAX_BYTES> {
    fn new(value: JsString) -> Result<Self, StringLimitExceeded> {
        let utf8_bytes = bounded_utf8_len(&value, MAX_BYTES)?;
        Ok(Self { value, utf8_bytes })
    }

    const fn utf8_bytes(&self) -> usize {
        self.utf8_bytes
    }

    fn into_string(self) -> String {
        String::from(self.value)
    }
}

struct BoundedJsFile {
    path: BoundedJsString<MAX_PLAYGROUND_PATH_BYTES>,
    content: BoundedJsString<MAX_PLAYGROUND_FILE_BYTES>,
}

/// A request whose count and all individual and aggregate string limits have
/// been checked without allocating Rust-owned request strings.
pub struct BoundedJsRequest {
    entry: BoundedJsString<MAX_PLAYGROUND_PATH_BYTES>,
    files: Vec<BoundedJsFile>,
}

impl BoundedJsRequest {
    pub fn decode(value: &JsValue) -> Result<Self, JsRequestBoundaryError> {
        require_object_shape(value, JsObjectSchema::Request)?;

        let raw_files = read_property(value, JsContainer::Request, JsProperty::Files)?;
        if !is_array(&raw_files, JsValueRole::Files)? {
            return Err(JsRequestShapeError::ExpectedArray {
                value: JsValueRole::Files,
            }
            .into());
        }
        let file_count_u32 = read_array_length(&raw_files)?;
        let file_count = usize::try_from(file_count_u32).map_err(|_| {
            JsRequestShapeError::InvalidArrayLength {
                value: JsValueRole::Files,
            }
        })?;
        validate_file_count(file_count)?;

        let entry = read_string_property(value, JsStringField::Entry)?;
        let entry = BoundedJsString::new(entry).map_err(|StringLimitExceeded| {
            ProjectValidationError::PathTooLong {
                role: ProjectPathRole::Entry,
                maximum: MAX_PLAYGROUND_PATH_BYTES,
            }
        })?;
        let mut budget = RequestSizeBudget::for_entry(entry.utf8_bytes())?;
        let mut files = Vec::with_capacity(file_count);

        for index in 0..file_count_u32 {
            let raw_file = read_property(
                &raw_files,
                JsContainer::Files,
                JsProperty::Element { index },
            )?;
            let rust_index =
                usize::try_from(index).map_err(|_| JsRequestShapeError::InvalidArrayLength {
                    value: JsValueRole::Files,
                })?;
            require_object_shape(&raw_file, JsObjectSchema::File { index: rust_index })?;

            let path =
                read_string_property(&raw_file, JsStringField::FilePath { index: rust_index })?;
            let path = BoundedJsString::new(path).map_err(|StringLimitExceeded| {
                ProjectValidationError::PathTooLong {
                    role: ProjectPathRole::File { index: rust_index },
                    maximum: MAX_PLAYGROUND_PATH_BYTES,
                }
            })?;

            let content =
                read_string_property(&raw_file, JsStringField::FileContent { index: rust_index })?;
            let content = BoundedJsString::new(content).map_err(|StringLimitExceeded| {
                ProjectValidationError::FileTooLarge {
                    index: rust_index,
                    maximum: MAX_PLAYGROUND_FILE_BYTES,
                }
            })?;

            budget = budget.with_file(rust_index, path.utf8_bytes(), content.utf8_bytes())?;
            files.push(BoundedJsFile { path, content });
        }

        Ok(Self { entry, files })
    }

    pub fn into_request(self) -> PlaygroundRequest {
        PlaygroundRequest {
            entry: self.entry.into_string(),
            files: self
                .files
                .into_iter()
                .map(|file| PlaygroundFile {
                    path: file.path.into_string(),
                    content: file.content.into_string(),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct StringLimitExceeded;

/// Opaque malformed-transport error exposed to the crate's JavaScript entry
/// point without widening the reflection model beyond this module.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct InvalidJsRequest(#[from] JsRequestShapeError);

/// Error classification at the raw JavaScript trust boundary.
#[derive(Debug, Error)]
pub enum JsRequestBoundaryError {
    /// A fixed transport field had the wrong JavaScript shape.
    #[error(transparent)]
    InvalidShape(InvalidJsRequest),
    /// A correctly shaped request exceeded a project policy limit.
    #[error(transparent)]
    Rejected(#[from] ProjectValidationError),
}

impl From<JsRequestShapeError> for JsRequestBoundaryError {
    fn from(error: JsRequestShapeError) -> Self {
        Self::InvalidShape(InvalidJsRequest(error))
    }
}

#[derive(Debug, Clone, Copy, Error)]
enum JsRequestShapeError {
    #[error("expected {value} to be a JavaScript object")]
    ExpectedObject { value: JsValueRole },
    #[error("expected {value} to be a JavaScript array")]
    ExpectedArray { value: JsValueRole },
    #[error("expected {value} to be a JavaScript string")]
    ExpectedString { value: JsValueRole },
    #[error("could not read {property} from {container}")]
    PropertyAccess {
        container: JsContainer,
        property: JsProperty,
    },
    #[error("could not inspect the properties of {value}")]
    PropertyInspection { value: JsValueRole },
    #[error("{value} has missing or unexpected properties")]
    UnexpectedProperties { value: JsValueRole },
    #[error("{value} has an invalid JavaScript array length")]
    InvalidArrayLength { value: JsValueRole },
}

#[derive(Debug, Clone, Copy)]
enum JsObjectSchema {
    Request,
    File { index: usize },
}

impl JsObjectSchema {
    const fn role(self) -> JsValueRole {
        match self {
            Self::Request => JsValueRole::Request,
            Self::File { index } => JsValueRole::File { index },
        }
    }

    const fn properties(self) -> [JsProperty; 2] {
        match self {
            Self::Request => [JsProperty::Entry, JsProperty::Files],
            Self::File { .. } => [JsProperty::Path, JsProperty::Content],
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum JsContainer {
    Request,
    Files,
    File { index: usize },
}

impl std::fmt::Display for JsContainer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request => formatter.write_str("the playground request"),
            Self::Files => formatter.write_str("the request's `files` array"),
            Self::File { index } => write!(formatter, "project file at index {index}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum JsProperty {
    Entry,
    Files,
    Element { index: u32 },
    Length,
    Path,
    Content,
}

impl JsProperty {
    fn key(self) -> JsValue {
        match self {
            Self::Entry => JsValue::from_str("entry"),
            Self::Files => JsValue::from_str("files"),
            Self::Element { index } => JsValue::from_f64(f64::from(index)),
            Self::Length => JsValue::from_str("length"),
            Self::Path => JsValue::from_str("path"),
            Self::Content => JsValue::from_str("content"),
        }
    }
}

impl std::fmt::Display for JsProperty {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entry => formatter.write_str("property `entry`"),
            Self::Files => formatter.write_str("property `files`"),
            Self::Element { index } => write!(formatter, "array element {index}"),
            Self::Length => formatter.write_str("property `length`"),
            Self::Path => formatter.write_str("property `path`"),
            Self::Content => formatter.write_str("property `content`"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum JsStringField {
    Entry,
    FilePath { index: usize },
    FileContent { index: usize },
}

impl JsStringField {
    const fn container(self) -> JsContainer {
        match self {
            Self::Entry => JsContainer::Request,
            Self::FilePath { index } | Self::FileContent { index } => JsContainer::File { index },
        }
    }

    const fn property(self) -> JsProperty {
        match self {
            Self::Entry => JsProperty::Entry,
            Self::FilePath { .. } => JsProperty::Path,
            Self::FileContent { .. } => JsProperty::Content,
        }
    }

    const fn role(self) -> JsValueRole {
        match self {
            Self::Entry => JsValueRole::Entry,
            Self::FilePath { index } => JsValueRole::FilePath { index },
            Self::FileContent { index } => JsValueRole::FileContent { index },
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum JsValueRole {
    Request,
    Files,
    File { index: usize },
    Entry,
    FilePath { index: usize },
    FileContent { index: usize },
}

impl std::fmt::Display for JsValueRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Request => formatter.write_str("the playground request"),
            Self::Files => formatter.write_str("property `files`"),
            Self::File { index } => write!(formatter, "project file at index {index}"),
            Self::Entry => formatter.write_str("property `entry`"),
            Self::FilePath { index } => {
                write!(
                    formatter,
                    "property `path` of project file at index {index}"
                )
            }
            Self::FileContent { index } => {
                write!(
                    formatter,
                    "property `content` of project file at index {index}"
                )
            }
        }
    }
}

fn require_object_shape(
    value: &JsValue,
    schema: JsObjectSchema,
) -> Result<(), JsRequestShapeError> {
    let role = schema.role();
    if !value.is_object() || is_array(value, role)? {
        return Err(JsRequestShapeError::ExpectedObject { value: role });
    }

    let keys = Reflect::own_keys(value)
        .map_err(|_| JsRequestShapeError::PropertyInspection { value: role })?;
    let expected = schema.properties();
    let key_count = usize::try_from(keys.length())
        .map_err(|_| JsRequestShapeError::PropertyInspection { value: role })?;
    if key_count != expected.len()
        || keys.iter().any(|key| {
            !expected
                .iter()
                .any(|property| Object::is(&key, &property.key()))
        })
    {
        return Err(JsRequestShapeError::UnexpectedProperties { value: role });
    }
    Ok(())
}

fn is_array(value: &JsValue, role: JsValueRole) -> Result<bool, JsRequestShapeError> {
    try_is_array(value).map_err(|_| JsRequestShapeError::PropertyInspection { value: role })
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp,
    reason = "the exact finite integer and u32 range checks precede conversion of JavaScript Array.length"
)]
fn read_array_length(array: &JsValue) -> Result<u32, JsRequestShapeError> {
    let value = read_property(array, JsContainer::Files, JsProperty::Length)?;
    let Some(length) = value.as_f64() else {
        return Err(JsRequestShapeError::InvalidArrayLength {
            value: JsValueRole::Files,
        });
    };
    if !length.is_finite()
        || length < 0.0
        || length > f64::from(u32::MAX)
        || length.trunc() != length
    {
        return Err(JsRequestShapeError::InvalidArrayLength {
            value: JsValueRole::Files,
        });
    }
    Ok(length as u32)
}

fn read_property(
    target: &JsValue,
    container: JsContainer,
    property: JsProperty,
) -> Result<JsValue, JsRequestShapeError> {
    Reflect::get(target, &property.key()).map_err(|_| JsRequestShapeError::PropertyAccess {
        container,
        property,
    })
}

fn read_string_property(
    target: &JsValue,
    field: JsStringField,
) -> Result<JsString, JsRequestShapeError> {
    let value = read_property(target, field.container(), field.property())?;
    value
        .dyn_into::<JsString>()
        .map_err(|_| JsRequestShapeError::ExpectedString {
            value: field.role(),
        })
}

/// Return the exact UTF-8 byte length produced by wasm-bindgen's JavaScript
/// string conversion, stopping as soon as the fixed limit is exceeded.
fn bounded_utf8_len(value: &JsString, max_bytes: usize) -> Result<usize, StringLimitExceeded> {
    let utf16_units = usize::try_from(value.length()).map_err(|_| StringLimitExceeded)?;
    // Every UTF-16 code unit contributes at least one UTF-8 byte, including
    // surrogate pairs (two units become four bytes) and unpaired surrogates
    // (which wasm-bindgen converts to the three-byte replacement character).
    if utf16_units > max_bytes {
        return Err(StringLimitExceeded);
    }

    let mut units = value.iter().peekable();
    let mut bytes = 0usize;
    while let Some(unit) = units.next() {
        let width = match unit {
            0x0000..=0x007F => 1,
            0x0080..=0x07FF => 2,
            0xD800..=0xDBFF
                if units
                    .peek()
                    .is_some_and(|next| (0xDC00..=0xDFFF).contains(next)) =>
            {
                units.next();
                4
            }
            _ => 3,
        };
        bytes = bytes
            .checked_add(width)
            .filter(|total| *total <= max_bytes)
            .ok_or(StringLimitExceeded)?;
    }
    Ok(bytes)
}
