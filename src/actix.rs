use actix_web::web::Bytes;
use actix_web::HttpMessage;
use lambda_http::Body;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

pub async fn run<F, I, S, B>(factory: F) -> Result<(), lambda_http::Error>
where
    F: Fn() -> I + Send + Clone + 'static,
    I: actix_service::IntoServiceFactory<S, actix_http::Request>,
    S: actix_service::ServiceFactory<actix_http::Request, Config = actix_web::dev::AppConfig>,
    S::Error: Into<actix_web::Error>,
    S::InitError: std::fmt::Debug,
    S::Response: Into<actix_http::Response<B>>,
    B: actix_web::body::MessageBody + Unpin,
{
    lambda_http::run(service(factory).await).await
}

pub struct ActixTowerService<'a, S, B>
where
    S: actix_service::Service<actix_http::Request> + 'a,
{
    service: Arc<S>,
    _phantom_b: PhantomData<B>,
    _phantom_a: PhantomData<&'a ()>,
}

pub async fn service<'a, F, I, S, B>(factory: F) -> ActixTowerService<'a, S::Service, B>
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
    let sf = factory().into_factory();

    let service = sf
        .new_service(actix_web::dev::AppConfig::default())
        .await
        .unwrap(); // TODO return instead of unwraping

    let service = std::sync::Arc::new(service);

    ActixTowerService {
        service,
        _phantom_a: PhantomData,
        _phantom_b: PhantomData,
    }
}

impl<'a, S, B> tower::Service<lambda_http::Request> for ActixTowerService<'a, S, B>
where
    S: actix_service::Service<actix_http::Request> + 'a,
    S::Response: Into<actix_http::Response<B>>,
    S::Error: Into<actix_web::Error>,
    B: actix_web::body::MessageBody + Unpin,
{
    type Response = lambda_http::Response<lambda_http::Body>;
    type Error = lambda_http::Error;

    type Future = TransformResponse<'a, S::Response, B, S::Error>;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: lambda_http::Request) -> Self::Future {
        let actix_req = http_to_actix_request(req);
        let fut = Box::pin(self.service.call(actix_req));

        TransformResponse {
            fut,
            _phantom: PhantomData,
        }
    }
}

/// Future that will convert an [`actix_http::Response`] into an actual [`lambda_http::Response`]
///
/// This is used by the `ActixTowerService` wrapper and is completely internal to the `service` function.
#[doc(hidden)]
pub struct TransformResponse<'a, R, B, E> {
    fut: Pin<Box<dyn Future<Output = Result<R, E>> + 'a>>,
    _phantom: PhantomData<B>,
}

impl<'a, R, E, B> Future for TransformResponse<'a, R, B, E>
where
    R: Into<actix_http::Response<B>>,
    E: Into<actix_web::Error>,
    B: actix_web::body::MessageBody + Unpin,
{
    type Output = Result<lambda_http::Response<lambda_http::Body>, lambda_http::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext) -> Poll<Self::Output> {
        match self.fut.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                let r = match result {
                    Ok(r) => actix_to_http_response(r.into()),
                    Err(err) => {
                        let e: actix_web::Error = err.into();

                        actix_to_http_response(e.error_response().into())
                    }
                };

                Poll::Ready(Ok(r))
            }
        }
    }
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
