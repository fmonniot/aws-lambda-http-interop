use axum::{routing::future::RouterFuture, Router};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub struct AxumService {
    router: Router,
}

pub async fn service(router: Router) -> AxumService {
    AxumService { router }
}

impl tower::Service<lambda_http::Request> for AxumService {
    type Response = lambda_http::Response<lambda_http::Body>;
    type Error = lambda_http::Error;

    type Future = TransformResponse;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.router
            .poll_ready(cx)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    fn call(&mut self, req: lambda_http::Request) -> Self::Future {
        let r = req.map(|b| match b {
            lambda_http::Body::Empty => hyper::Body::empty(),
            lambda_http::Body::Text(s) => s.into(),
            lambda_http::Body::Binary(b) => b.into(),
        });
        let fut = Box::new(self.router.call(r));

        TransformResponse::WaitResponse { fut }
    }
}

// To get autocompletion with rust-analyzer, set the option
// rust-analyzer.experimental.procAttrMacros to true
pin_project_lite::pin_project! {
    /// Future that will convert the hyper response body to the lambda_http's body
    ///
    /// This is used by the `AxumService` wrapper and is completely internal to the `service` function.
    #[project = TransformProj]
    #[doc(hidden)]
    pub enum TransformResponse {
        WaitResponse{ #[pin] fut: Box<RouterFuture<hyper::Body>> },
        WaitBody {
            parts: Option<http::response::Parts>,
            body: Pin<Box<dyn Future<Output = Result<axum::body::Bytes, axum::Error>>>>
        }
    }
}

impl Future for TransformResponse {
    type Output = Result<lambda_http::Response<lambda_http::Body>, lambda_http::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        match self.as_mut().project() {
            TransformProj::WaitResponse { fut } => {
                match fut.poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(result) => {
                        let response = result.unwrap(); // TODO

                        let (parts, body) = response.into_parts();
                        let parts = Some(parts);
                        let body = Box::pin(hyper::body::to_bytes(body));

                        // We got the response, switching to next polling phase: getting the body
                        self.set(TransformResponse::WaitBody { parts, body });

                        // TODO Do we need to wake up the waker ?
                        Poll::Pending
                    }
                }
            }
            TransformProj::WaitBody { parts, body } => {
                match body.as_mut().poll(cx) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(body) => {
                        let bytes = body.unwrap(); // TODO
                        let body = lambda_http::Body::Binary(bytes.to_vec());

                        let parts = parts.take().expect("parts cannot be None");

                        let response = lambda_http::Response::from_parts(parts, body);

                        Poll::Ready(Ok(response))
                    }
                }
            }
        }
    }
}
