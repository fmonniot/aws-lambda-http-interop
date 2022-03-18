use actix_web::web::Bytes;
use actix_web::HttpMessage;
use lambda_http::Body;

// TODO Return a proper type instead of an impl
// And then impl Send with the proper bound. Let's try it this way and see
// if we can make the hyper server happy.
pub async fn as_service<'a, F, I, S, B>(
    factory: F,
) -> Result<
    impl tower::Service<
            lambda_http::Request,
            Response = lambda_http::Response<lambda_http::Body>,
            Error = lambda_http::Error,
        > + 'a,
    lambda_http::Error,
>
where
    F: Fn() -> I + Send + Clone + 'static,
    I: actix_service::IntoServiceFactory<S, actix_http::Request>,
    S: actix_service::ServiceFactory<actix_http::Request, Config = actix_web::dev::AppConfig>,
    S::Error: Into<actix_web::Error>,
    S::InitError: std::fmt::Debug,
    S::Response: Into<actix_http::Response<B>>,
    S::Service: 'a,
    B: actix_web::body::MessageBody,
{
    use actix_service::Service;

    let sf = factory().into_factory();

    let svc = sf
        .new_service(actix_web::dev::AppConfig::default())
        .await
        .unwrap(); // TODO return instead of unwraping

    let svc = std::sync::Arc::new(svc);

    let t_svc = tower::service_fn(move |req| {
        let svc = svc.clone();
        async move {
            let actix_req = http_to_actix_request(req);

            let r = svc.call(actix_req).await;

            let r = match r {
                Ok(r) => actix_to_http_response(r.into()),
                Err(err) => {
                    let e: actix_web::Error = err.into();

                    actix_to_http_response(e.error_response().into())
                }
            };

            Ok(r)
        }
    });

    Ok(t_svc)
}

/// This is the signature for creating an actix-web server. The various factories are
/// probably there because the server is multi-threaded. We are keepin the same signature
/// to make interopability easier, even though most of the factory aren't required.
pub async fn run<F, I, S, B>(factory: F) -> Result<(), lambda_http::Error>
where
    F: Fn() -> I + Send + Clone + 'static,
    I: actix_service::IntoServiceFactory<S, actix_http::Request>,
    S: actix_service::ServiceFactory<actix_http::Request, Config = actix_web::dev::AppConfig>,
    S::Error: Into<actix_web::Error>,
    S::InitError: std::fmt::Debug,
    S::Response: Into<actix_http::Response<B>>,
    B: actix_web::body::MessageBody,
{
    lambda_http::run(as_service(factory).await?).await
}

fn http_to_actix_request(req: lambda_http::Request) -> actix_http::Request {
    let (
        http::request::Parts {
            method,
            uri,
            version,
            headers,
            mut extensions,
            ..
        },
        body,
    ) = req.into_parts();

    use actix_http::{BoxedPayloadStream, Payload};

    // We start by transforming the lambda request body into an actix one.
    let payload: Payload<BoxedPayloadStream> = {
        let (_, mut payload) = actix_http::h1::Payload::create(true);

        let b = match body {
            lambda_http::Body::Empty => Bytes::default(),
            lambda_http::Body::Text(s) => Bytes::from(s),
            lambda_http::Body::Binary(b) => Bytes::from(b),
        };

        payload.unread_data(b);

        payload.into()
    };

    // Then we move the Parts from http to actix
    let mut actix_request = actix_http::Request::with_payload(payload);
    let head = actix_request.head_mut();
    head.method = method;
    head.uri = uri;
    head.version = version;
    head.headers = headers.into();

    // And finally set some extensions. We ignore the query/params/stage extensions as
    // they should already be present in the uri.

    let mut r_ext = actix_request.extensions_mut();

    if let Some(aws_context) = extensions.remove::<lambda_runtime::Context>() {
        r_ext.insert(aws_context);
    }

    if let Some(req_context) = extensions.remove::<lambda_http::request::RequestContext>() {
        r_ext.insert(req_context);
    }

    // We are done inserting extensions, release reference to it
    drop(r_ext);

    actix_request
}

fn actix_to_http_response<B: actix_web::body::MessageBody>(
    res: actix_http::Response<B>,
) -> lambda_http::Response<lambda_http::Body> {
    // The ResponseHead/Parts is gonna be relatively simple
    // The body is going to be way more interesting, mostly
    // because actix MessageBody can be a Stream.
    // Perhaps let the stream on the side for a first pass
    // and come back to it later down the road ?
    // Do note that AWS lambda do not support chunked/stream
    // responses, so we could accumulate the content of the
    // stream in memory to build the correct lambda_http::Body

    let (head, body) = res.into_parts();

    let mut builder = lambda_http::Response::builder().status(head.status());

    // TODO Consider using head.headers_mut().drain() to avoid cloning the headers
    for (name, value) in head.headers() {
        builder = builder.header(name, value);
    }

    let b = match body.size() {
        actix_http::body::BodySize::None => Body::Empty,
        _ => {
            // TODO Do we need to set the correct Content-Length header ?

            match body.try_into_bytes() {
                Ok(bytes) => {
                    // TODO how do we decide between Body::String and Body::Binary ?
                    Body::Binary(bytes.to_vec())
                }
                Err(_body) => {
                    // TODO poll the body and accumulate its content here
                    todo!("We do not support streamed response yet")
                }
            }
        }
    };

    builder
        .body(b)
        .expect("actix to http response conversion should not fail")
}
