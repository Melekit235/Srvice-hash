use actix_multipart::Multipart;
use actix_web::{middleware, web, App, Error, HttpResponse, HttpServer, Result};
pub use actix_web::web::Path;

use futures_util::TryStreamExt as _;
use openssl::sha::Sha256;

use serde::Deserialize;
use serde::Serialize;

use hex;

use tokio::task;
use tokio::sync::mpsc::channel;

use utoipa::ToSchema;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use base64::prelude::*;

use std::collections::HashMap;
use std::sync::Arc;
use std::env;
use std::fs::File;
use uuid::Uuid;

use tokio::sync::RwLock;
use std::io::Read;


const CHUNK_SIZE: usize = 8192;

#[derive(Serialize, ToSchema)]
struct HashFileResponse {
    hash: String,
}

#[derive(Serialize, ToSchema)]
struct Base64FileResponse {
    base64_content: String,
}

#[derive(Serialize, ToSchema)]
struct FileStatus {
    id: String,
    location: String,
    hash: Option<String>,
    status: String,
}

#[derive(Deserialize, ToSchema)]
struct FilePath {
    path: String,
}

type FileDatabase = Arc<RwLock<HashMap<String, FileStatus>>>;

#[utoipa::path(
    post,
    path = "/upload",
    request_body(
        content = FilePath, 
        description = "The file path to be uploaded",
        content_type = "application/json",
    ),
    responses(
        (status = 200, description = "File path uploaded and processing", body = FileStatus),
        (status = 400, description = "POST without data")
    ),
    tag = "hash",
)]
async fn upload_file(file_path: web::Json<FilePath>, db: web::Data<FileDatabase>) -> Result<HttpResponse, Error> {
    let file_id = Uuid::new_v4().to_string();
    let file_status = FileStatus {
        id: file_id.clone(),
        location: file_path.path.clone(),
        hash: None,
        status: "processing".to_string(),
    };

    {
        let mut db_write = db.write().await;
        db_write.insert(file_id.clone(), file_status);
    }

    let db_clone = db.clone();
    let file_path_clone = file_path.path.clone();
    let file_id_clone = file_id.clone();
    task::spawn(async move {
        let mut hasher = Sha256::new();
        let mut file = match File::open(&file_path_clone) {
            Ok(file) => file,
            Err(_) => {
                let mut db_write = db_clone.write().await;
                if let Some(file_status) = db_write.get_mut(&file_id_clone) {
                    file_status.status = "error: file not found".to_string();
                }
                return;
            }
        };
        let mut buffer = [0u8; CHUNK_SIZE];

        loop {
            let n = match file.read(&mut buffer) {
                Ok(n) if n == 0 => break,
                Ok(n) => n,
                Err(_) => {
                    let mut db_write = db_clone.write().await;
                    if let Some(file_status) = db_write.get_mut(&file_id_clone) {
                        file_status.status = "error: failed to read file".to_string();
                    }
                    return;
                }
            };
            hasher.update(&buffer[..n]);
        }

        let hash = hex::encode(hasher.finish());

        let mut db_write = db_clone.write().await;
        if let Some(file_status) = db_write.get_mut(&file_id_clone) {
            file_status.hash = Some(hash);
            file_status.status = "ready".to_string();
        }
    });

    let db_read = db.read().await;
    let file_status = db_read.get(&file_id).unwrap();


    Ok(HttpResponse::Ok().json(file_status))
}

#[utoipa::path(
    get,
    path = "/status/{file_id}",
    responses(
        (status = 200, description = "File in process", body = FileStatus),
        (status = 200, description = "File hashed", body = HashFileResponse),
        (status = 404, description = "File not found")
    ),
    tag = "hash",
)]
async fn get_file_status(path: web::Path<String>, db: web::Data<FileDatabase>) -> Result<HttpResponse, Error> {

    let file_id = path.into_inner();

    let mut db_write = db.write().await;
    if let Some(file_status) = db_write.get(&file_id) {
        if file_status.status == "ready" {
            let hash = file_status.hash.clone().unwrap();
            db_write.remove(&file_id);
            Ok(HttpResponse::Ok().json(HashFileResponse { hash }))
        } else {
            Ok(HttpResponse::Ok().json(file_status))
        }
    } else {
        Ok(HttpResponse::NotFound().body("File ID not found"))
    }

}

#[utoipa::path(
    post,
    path = "/encode",
    request_body(
        content = Multipart, 
        description = "The file to be encode",
        content_type = "multipart/form-data",
    ),
    responses(
        (status = 200, description = "Base64 encoded file content", body = Base64FileResponse),
        (status = 400, description = "POST without data")
    ),
    tag = "encode",
)]
async fn encode_base64(mut payload: Multipart) -> Result<HttpResponse, Error> {
    
    let (tx, mut rx) = channel::<Vec<u8>>(10);

    let encode_handle = task::spawn(async move {
        let mut base64_content = String::new();
        while let Some(data) = rx.recv().await {

            base64_content.push_str(BASE64_STANDARD.encode(data).as_str()) ;
        }
        base64_content
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

    let base64_content = encode_handle.await.unwrap();

    Ok(HttpResponse::Ok().json(Base64FileResponse { base64_content }))
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

#[derive(OpenApi)]
#[openapi(paths(hash_file, encode_base64, upload_file, get_file_status), components(schemas(HashFileResponse, Base64FileResponse, FileStatus, FilePath)))]
struct ApiDoc;


#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let host = env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let port: u16 = env::var("PORT").unwrap_or_else(|_| "8080".to_string()).parse().unwrap_or(8080);

    log::info!("starting HTTP server at http://{}:{}", host, port);

    let db: FileDatabase = Arc::new(RwLock::new(HashMap::new()));

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(db.clone()))
            .wrap(middleware::Logger::default())
            .service(
                web::resource("/hashfile")
                    .route(web::post().to(hash_file)),
            )
            .service(
                SwaggerUi::new("/swagger/{_:.*}")
                    .url("/api-docs/openapi.json", ApiDoc::openapi()),
            )
            .service(
                web::resource("/encode")
                    .route(web::post().to(encode_base64))
            )            
            .service(
                web::resource("/upload")
                    .route(web::post().to(upload_file)),
            )
            .service(
                web::resource("/status/{file_id}")
                    .route(web::get().to(get_file_status)),
            )
            .service(
                SwaggerUi::new("/swagger/{_:.*}")
                    .url("/api-docs/openapi.json", ApiDoc::openapi()),
            )
            .service(actix_files::Files::new("/", "./static").index_file("index.html"))
    })
    .bind((host.as_str(), port))?
    .workers(2)
    .run()
    .await
}