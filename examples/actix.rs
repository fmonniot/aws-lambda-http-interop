
// You'll have to run this example in a lambda aware environment.
// Otherwise the lambda runtime will complain that it is missing
// its configuration.
/*
# If you are not on linux, and you got the right toolchain setup (I don't quite remember how I did it, so no much help from me there)
$ export TARGET_CC=x86_64-linux-musl-gcc
$ cargo build --release --example actix --target x86_64-unknown-linux-musl
$ cp target/x86_64-unknown-linux-musl/release/examples/actix ./bootstrap
$ cat examples/alb_request.json | docker run --rm -v "$PWD":/var/task:ro,delegated -i -e DOCKER_LAMBDA_USE_STDIN=1 lambci/lambda:provided
*/
#[tokio::main]
async fn main() {

    aws_lambda_http_interop::actix::run(|| {
        actix_web::App::new().route("/hey", actix_web::web::get().to(manual_hello))
    })
    .await
    .unwrap();
}

async fn manual_hello() -> impl actix_web::Responder {
    actix_web::HttpResponse::Ok().body("Hey there!")
}
