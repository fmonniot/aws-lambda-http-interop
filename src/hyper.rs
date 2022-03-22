use hyper::service::{make_service_fn, service_fn};
use hyper::{Request, Response, Server};
use std::convert::Infallible;
use std::future::Future;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tower::Service;

// Should this return a Server instead ? Will I be able to make it work with the generics ?
pub async fn run<'a, F, Fut, R, S>(factory: F) -> Result<(), hyper::Error>
where
    F: Fn() -> Fut + Send + Clone + 'static,
    Fut: Future<Output = S> + Send,
    S: tower::Service<lambda_http::Request, Response = R, Error = lambda_http::Error>
        + Send
        + 'static,
    S::Future: Send + 'a,
    R: lambda_http::IntoResponse,
{
    // We'll bind to 127.0.0.1:3000
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));

    let make = MakeLambdaService {
        factory: factory.clone(),
    };

    // A `Service` is needed for every connection, so this
    // creates one from our `hello_world` function.
    let make_svc = make_service_fn(move |_conn| {
        let factory = factory.clone();

        async move {
            // service_fn converts our function into a `Service`
            Ok::<_, Infallible>(service_fn(move |req: Request<hyper::Body>| {
                let factory = factory.clone();
                async move {
                    let factory = factory();
                    let mut handler = factory.await;
                    let (parts, body) = req.into_parts();
                    let body = hyper::body::to_bytes(body).await?;
                    let body = lambda_http::Body::Binary(body.to_vec());
                    let lambda_http_req = Request::from_parts(parts, body);

                    let (parts, body) = handler
                        .call(lambda_http_req)
                        .await
                        .expect("TODO Something with errors here")
                        .into_response()
                        .into_parts();

                    let body = match body {
                        lambda_http::Body::Empty => hyper::Body::empty(),
                        lambda_http::Body::Text(s) => hyper::Body::from(s),
                        lambda_http::Body::Binary(v) => hyper::Body::from(v),
                    };

                    let res: Result<Response<hyper::Body>, hyper::Error> =
                        Ok(Response::from_parts(parts, body));

                    res
                }
            }))
        }
    });

    let server = Server::bind(&addr).serve(make);

    // Run the server
    server.await
}

#[doc(hidden)]
pub struct MakeLambdaService<F> {
    factory: F,
}

// TODO Is the &'t lifetime useful given we don't make use of Target ?
impl<'a, 't, F, Fut, Svc, R, Target, MkErr> Service<&'t Target> for MakeLambdaService<F>
where
    F: Fn() -> Fut + Send + Clone, // TODO Send and Clone might not be useful
    Fut: Future<Output = Result<LambdaService<'a, Svc, R>, MkErr>>,
    Svc: Service<lambda_http::Request, Response = R, Error = lambda_http::Error> + Send,
    Svc::Future: Send + 'a,
    R: lambda_http::IntoResponse,
    MkErr: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Error = MkErr;
    type Response = LambdaService<'a, Svc, R>;
    type Future = Fut;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _target: &'t Target) -> Self::Future {
        (self.factory)()
    }
}

#[doc(hidden)]
struct LambdaService<'a, S, R>
where
    S: tower::Service<lambda_http::Request, Response = R, Error = lambda_http::Error> + Send,
    S::Future: Send + 'a,
    R: lambda_http::IntoResponse,
{
    service: S,
    _phantom_a: PhantomData<&'a ()>,
}

impl<'a, S,R> Service<Request<hyper::Body>> for LambdaService<'a, S, R>
where
    S: tower::Service<lambda_http::Request, Response = R, Error = lambda_http::Error> + Send,
    S::Future: Send + 'a,
    R: lambda_http::IntoResponse,
{
    type Response = Response<hyper::Body>;
    type Error = lambda_http::Error;

    type Future = TransformResponse<'a, R, lambda_http::Error>;

    fn poll_ready(&mut self, _cx: &mut std::task::Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<hyper::Body>) -> Self::Future {
        let req = hyper_to_lambda_request(req);
        let fut = Box::pin(self.service.call(req));

        TransformResponse { fut }
    }
}

/// Future that will convert a [`lambda_http::Response`] into a [`hyper::Response`]
struct TransformResponse<'a, R, E> {
    fut: Pin<Box<dyn Future<Output = Result<R, E>> + Send + 'a>>,
}

impl<'a, R, E> Future for TransformResponse<'a, R, E>
where
    R: lambda_http::IntoResponse,
    E: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Output = Result<hyper::Response<hyper::Body>, E>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext) -> Poll<Self::Output> {
        match self.fut.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                Poll::Ready(result.map(|r| lambda_to_hyper_response(r.into_response())))
            }
        }
    }
}

fn lambda_to_hyper_response(
    r: lambda_http::Response<lambda_http::Body>,
) -> hyper::Response<hyper::Body> {
    let (parts, body) = r.into_parts();

    let body = match body {
        lambda_http::Body::Empty => hyper::Body::empty(),
        lambda_http::Body::Text(s) => hyper::Body::from(s),
        lambda_http::Body::Binary(v) => hyper::Body::from(v),
    };

    Response::from_parts(parts, body)
}

fn hyper_to_lambda_request(r: hyper::Request<hyper::Body>) -> lambda_http::Request {
    /*
    let (parts, body) = req.into_parts();
    let body = hyper::body::to_bytes(body).await?; // TODO needs to bridge that async in the manual world
    let body = lambda_http::Body::Binary(body.to_vec());

    Request::from_parts(parts, body)
    */
    todo!()
}
