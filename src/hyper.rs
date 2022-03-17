use hyper::service::{make_service_fn, service_fn};
use hyper::{Request, Response, Server};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Mutex;

// Should this return a Server instead ? Will I be able to make it work with the generics ?
pub async fn run<'a, R, S>(handler: S) -> Result<(), hyper::Error>
where
    S: tower::Service<lambda_http::Request, Response = R, Error = lambda_http::Error>
        + Send
        + 'static,
    S::Future: Send + 'a,
    R: lambda_http::IntoResponse,
{
    // We'll bind to 127.0.0.1:3000
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let handler = Arc::new(Mutex::new(handler));

    // A `Service` is needed for every connection, so this
    // creates one from our `hello_world` function.
    let make_svc = make_service_fn(move |_conn| {
        let handler = handler.clone();

        async move {
            // service_fn converts our function into a `Service`
            Ok::<_, Infallible>(service_fn(move |req: Request<hyper::Body>| {
                let handler = handler.clone();
                async move {
                    let (parts, body) = req.into_parts();
                    let body = hyper::body::to_bytes(body).await?;
                    let body = lambda_http::Body::Binary(body.to_vec());
                    let lambda_http_req = Request::from_parts(parts, body);

                    let mut handler = handler.lock().await;
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

    let server = Server::bind(&addr).serve(make_svc);

    // Run the server
    server.await
}
