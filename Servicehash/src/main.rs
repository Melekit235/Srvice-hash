use actix_multipart::Multipart;
use actix_web::{middleware, web, App, Error, HttpResponse, HttpServer};

use futures_util::TryStreamExt as _;
use openssl::sha::Sha256;

use serde::Serialize;

use hex;

use tokio::task;
use tokio::sync::mpsc::channel;

use utoipa::ToSchema;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

const CHUNK_SIZE: usize = 8192;

#[derive(Serialize, ToSchema)]
struct HashFileResponse {
    hash: String,
}



#[utoipa::path(
    post,
    path = "/hashfile",
    request_body(
        content = Multipart, 
        description = "The file to be hashed",
        content_type = "multipart/form-data",
    ),
    responses(
        (status = 200, description = "Hash of the file", body = HashFileResponse),
        (status = 400, description = "POST without data")
    ),
    tag = "hash",
)]
async fn hash_file(mut payload: Multipart) -> Result<HttpResponse, Error> {
    let (tx, mut rx) = channel::<Vec<u8>>(10);
    let hash_handle = task::spawn(async move {
        let mut hasher = Sha256::new();
        while let Some(data) = rx.recv().await {
            hasher.update(&data);
        }
        hasher.finish()
    });

    while let Some(mut field) = payload.try_next().await? {
        let mut buffer = Vec::with_capacity(CHUNK_SIZE);

        while let Some(chunk) = field.try_next().await? {
            buffer.extend_from_slice(&chunk);

            if buffer.len() == CHUNK_SIZE {
                let write_buffer = buffer.split_off(0);
                tx.send(write_buffer).await.unwrap();
            }
        }

        if !buffer.is_empty() {
            tx.send(buffer).await.unwrap();
        }
    }

    drop(tx);

    let result = hash_handle.await.unwrap();
    let hash_hex = hex::encode(result);

    Ok(HttpResponse::Ok().json(HashFileResponse { hash: hash_hex }))
}

#[utoipa::path(
    get,
    path = "/hashfile",
    responses(
        (status = 200, description = "HTML upload form")
    ),
    tag = "index"
)]
async fn index() -> HttpResponse {
    let html = r#"<html>
        <head><title>Upload Test</title></head>
        <body>
            <form target="/" method="post" enctype="multipart/form-data">
                <input type="file" multiple name="file"/>
                <button type="submit">Submit</button>
            </form>
        </body>
    </html>"#;

    HttpResponse::Ok().body(html)
}

#[derive(OpenApi)]
#[openapi(paths(index, hash_file), components(schemas( HashFileResponse)))]
struct ApiDoc;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    log::info!("starting HTTP server at http://localhost:8080");

    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .service(
                web::resource("/hashfile")
                    .route(web::get().to(index))
                    .route(web::post().to(hash_file)),
            )
            .service(
                SwaggerUi::new("/swagger/{_:.*}")
                    .url("/api-docs/openapi.json", ApiDoc::openapi()),
            )
    })
    .bind(("127.0.0.1", 8080))?
    .workers(2)
    .run()
    .await
}
