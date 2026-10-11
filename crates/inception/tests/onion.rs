use inception::InceptionError as _;

#[inception::error]
enum TransportError {
    #[code("transport.timeout")]
    #[description("The transport timed out.")]
    Timeout,
}

#[inception::error]
enum ProviderError {
    #[code("provider.request_failed")]
    #[description("The provider request failed.")]
    RequestFailed {
        #[caused_by(inception)]
        #[from]
        error: TransportError,
    },
}

#[inception::error]
enum SendError {
    #[code("send.provider_unavailable")]
    #[description("The mail provider is unavailable.")]
    ProviderUnavailable {
        #[caused_by(inception)]
        error: ProviderError,
    },
}

#[inception::error]
enum ExternalError {
    #[code("external.io")]
    #[description("External I/O failed.")]
    Io {
        #[caused_by(inception)]
        source: inception::Inceptioned<std::io::Error>,
    },
}

#[inception::error]
enum BoxedExternalError {
    #[code("external.dependency")]
    #[description("A boxed external dependency failed.")]
    Dependency {
        #[caused_by]
        error: Box<dyn std::error::Error + Send + Sync + 'static>,
    },
}

#[inception::error]
#[derive(Clone, PartialEq, Eq)]
enum ClassifiedError {
    #[code("classified.failed")]
    #[description("The classified operation failed.")]
    Failed {
        resource: String,
        attempt: u32,
        #[hide]
        credential: Secret,
        unclassified: String,
    },
}

#[derive(Clone, PartialEq, Eq)]
struct Secret;

#[inception::error]
enum ImplicitDescriptionError {
    Failed,
}

fn load_external() -> Result<(), std::io::Error> {
    Err(std::io::Error::other("private"))
}

#[test]
fn descriptions_are_optional() {
    assert_eq!(
        ImplicitDescriptionError::failed().code(),
        "ImplicitDescriptionError.Failed"
    );
    assert_eq!(ImplicitDescriptionError::failed().description(), "Failed");
}

fn trace_length(error: &dyn inception::InceptionError) -> usize {
    let mut length = 1;
    let mut error = error;
    while let Some(nested) = error.nested_error() {
        length += 1;
        error = nested;
    }
    length
}

#[test]
fn nested_errors_retain_codes_descriptions_sources_and_creation_sites() {
    let transport_line = line!() + 1;
    let transport = TransportError::timeout();
    let provider_line = line!() + 1;
    let provider = inception::locate!(ProviderError::RequestFailed { error: transport });
    let send_line = line!() + 1;
    let send = SendError::provider_unavailable(provider);

    let provider = send.nested_error().unwrap();
    let transport = provider.nested_error().unwrap();
    assert_eq!(trace_length(&send), 3);
    assert_eq!(send.code(), "send.provider_unavailable");
    assert_eq!(provider.code(), "provider.request_failed");
    assert_eq!(transport.code(), "transport.timeout");
    assert_eq!(send.description(), "The mail provider is unavailable.");
    assert_eq!(send.created_at().line(), send_line);
    assert_eq!(provider.created_at().line(), provider_line);
    assert_eq!(transport.created_at().line(), transport_line);
    assert!(std::error::Error::source(&send).is_some());

    let trace = send.trace().to_string();
    assert!(trace.contains("send.provider_unavailable"));
    assert!(trace.contains("provider.request_failed"));
    assert!(trace.contains("transport.timeout"));
    assert!(trace.contains(file!()));
}

#[test]
fn from_conversion_creates_a_new_semantic_layer() {
    fn wrap() -> Result<(), ProviderError> {
        Err(TransportError::timeout())?;
        Ok(())
    }

    let error = wrap().unwrap_err();
    let nested = error.nested_error().unwrap();
    assert_eq!(trace_length(&error), 2);
    assert_eq!(error.code(), "provider.request_failed");
    assert_eq!(nested.code(), "transport.timeout");
    assert_ne!(error.created_at(), nested.created_at());
}

#[test]
fn external_sources_become_located_inception_layers() {
    let error = ExternalError::io(inception::Inceptioned::new(std::io::Error::other(
        "private diagnostic",
    )));

    assert_eq!(trace_length(&error), 2);
    assert_eq!(error.code(), "external.io");
    assert_eq!(error.to_string(), "External I/O failed.");
    assert!(!format!("{error:?}").contains("private diagnostic"));
    assert_eq!(
        std::error::Error::source(&error)
            .and_then(std::error::Error::source)
            .map(ToString::to_string),
        Some("private diagnostic".to_owned())
    );
}

#[test]
fn boxed_trait_object_sources_are_supported_and_hidden() {
    let error =
        BoxedExternalError::dependency(Box::new(std::io::Error::other("private boxed diagnostic")));

    assert_eq!(error.code(), "external.dependency");
    assert_eq!(trace_length(&error), 1);
    assert!(!format!("{error:?}").contains("private boxed diagnostic"));
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("private boxed diagnostic".to_owned())
    );
}

#[test]
fn entries_are_automatic_and_hidden_values_do_not_require_display() {
    let error = ClassifiedError::failed(
        "mailbox-1".to_owned(),
        3,
        Secret,
        "also not classified".to_owned(),
    );
    let cloned = error.clone();
    assert_eq!(error, cloned);

    let fields = error
        .entries()
        .into_iter()
        .map(|entry| (entry.key().to_owned(), entry.value().to_owned()))
        .collect::<Vec<_>>();
    assert_eq!(
        fields,
        vec![
            ("resource".to_owned(), "mailbox-1".to_owned()),
            ("attempt".to_owned(), "3".to_owned()),
            ("credential".to_owned(), "<hidden>".to_owned()),
            ("unclassified".to_owned(), "also not classified".to_owned()),
        ]
    );

    let debug = format!("{error:?} {:?}", error.kind());
    assert!(!debug.contains("never print me"));
    assert!(!debug.contains("also not classified"));
    assert!(!debug.contains("mailbox-1"));

    let entries = error
        .entries()
        .into_iter()
        .map(|entry| (entry.key(), entry.value().to_owned(), entry.is_hidden()))
        .collect::<Vec<_>>();
    assert_eq!(entries[2], ("credential", "<hidden>".to_owned(), true));
}

#[test]
fn builders_and_locate_err_cover_common_call_sites() {
    let missing = ClassifiedError::failed_builder().build().unwrap_err();
    assert_eq!(missing.field(), "resource");

    let built = ClassifiedError::failed_builder()
        .resource("resource-1".to_owned())
        .attempt(1)
        .credential(Secret)
        .unclassified("diagnostic".to_owned())
        .build()
        .unwrap();
    assert_eq!(built.code(), "classified.failed");

    let error = load_external()
        .map_err(inception::locate_err!(ExternalError::Io))
        .unwrap_err();
    assert_eq!(error.code(), "external.io");
    assert!(error.trace().to_string().contains("external.io"));

    let located = ExternalError::io(inception::Inceptioned::new(std::io::Error::other(
        "private",
    )))
    .locate();
    assert_eq!(located.code(), "external.io");
}

#[test]
fn locate_wraps_ordinary_errors_as_inception_layers() {
    let location_line = line!() + 1;
    let error = inception::locate!(std::io::Error::other("private"));
    assert_eq!(error.code(), "inception.external");
    assert_eq!(error.created_at().line(), location_line);
    let trace = error.trace().to_string();
    // std::io::error::Error on older toolchains, core::io::error::Error on newer ones
    assert!(trace.contains("io::error::Error"));
    assert!(!trace.contains("private"));
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("private".to_owned())
    );
}

#[test]
fn bail_err_returns_a_located_ordinary_error() {
    fn fail() -> Result<(), inception::Inceptioned<std::io::Error>> {
        inception::bail_err!(std::io::Error::other("private"));
    }

    let error = fail().unwrap_err();
    assert_eq!(error.code(), "inception.external");
    assert_eq!(
        std::error::Error::source(&error).map(ToString::to_string),
        Some("private".to_owned())
    );
}

#[test]
fn semantic_equality_ignores_diagnostic_creation_coordinates() {
    let first = ClassifiedError::failed("mailbox-1".to_owned(), 3, Secret, "opaque".to_owned());
    let second = ClassifiedError::failed("mailbox-1".to_owned(), 3, Secret, "opaque".to_owned());
    assert_ne!(first.created_at(), second.created_at());
    assert_eq!(first, second);
}

#[test]
fn catalog_exposes_stable_error_classes() {
    assert_eq!(ClassifiedError::CATALOG.len(), 1);
    assert_eq!(ClassifiedError::CATALOG[0].code(), "classified.failed");
    assert_eq!(
        ClassifiedError::CATALOG[0].description(),
        "The classified operation failed."
    );

    let error = ClassifiedError::failed("mailbox-1".to_owned(), 3, Secret, "opaque".to_owned());
    assert_eq!(error.code(), "classified.failed");
}
