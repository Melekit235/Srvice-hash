use actix_multipart::Multipart;
use actix_web::{middleware, web, App, Error, HttpResponse, HttpServer};
use futures_util::TryStreamExt as _;
use openssl::sha::Sha256;
//use openssl::hash::{hash, MessageDigest};
use hex;

use tokio::task;
use tokio::sync::mpsc::channel;

const CHUNK_SIZE: usize = 8192;

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

    Ok(HttpResponse::Ok().body(hash_hex))
}


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

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    log::info!("creating temporary upload directory");
    std::fs::create_dir_all("./tmp")?;

    log::info!("starting HTTP server at http://localhost:8080");

    HttpServer::new(|| {
        App::new()
            .wrap(middleware::Logger::default())
            .service(
                web::resource("/hashfile")
                    .route(web::get().to(index))
                    .route(web::post().to(hash_file)),
            )
    })
    .bind(("127.0.0.1", 8080))?
    .workers(2)
    .run()
    .await
}