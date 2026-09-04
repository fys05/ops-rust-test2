use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;

async fn test_app() -> axum::Router {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &std::env::var("TEST_DATABASE_URL")
                .unwrap_or_else(|_| "postgres://postgres:postgres@127.0.0.1:5432/postgres".into()),
        )
        .await
        .unwrap();

    sqlx::query("DROP TABLE IF EXISTS students")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DROP TABLE IF EXISTS classes")
        .execute(&pool)
        .await
        .unwrap();

    ops_rust_test2::init_db(&pool).await.unwrap();
    ops_rust_test2::app(pool)
}

async fn request_json(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn class_can_be_created_and_listed() {
    let app = test_app().await;

    let (status, created) = request_json(
        app.clone(),
        "POST",
        "/api/classes",
        json!({"name":"三年级一班","teacher":"王老师","room":"301"}),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["name"], "三年级一班");
    assert_eq!(created["teacher"], "王老师");
    assert_eq!(created["room"], "301");
    assert!(created["id"].as_i64().unwrap() > 0);

    let (status, list) = get_json(app, "/api/classes").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "三年级一班");
}

#[tokio::test]
async fn students_are_managed_under_their_class() {
    let app = test_app().await;
    let (_, class) = request_json(
        app.clone(),
        "POST",
        "/api/classes",
        json!({"name":"四年级二班","teacher":"李老师","room":"402"}),
    )
    .await;
    let class_id = class["id"].as_i64().unwrap();

    let (status, student) = request_json(
        app.clone(),
        "POST",
        &format!("/api/classes/{class_id}/students"),
        json!({"name":"张三","age":10,"gender":"男","phone":"13800000000"}),
    )
    .await;

    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(student["name"], "张三");
    assert_eq!(student["class_id"], class_id);

    let (status, students) = get_json(app, &format!("/api/classes/{class_id}/students")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(students.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn creating_student_for_missing_class_returns_not_found() {
    let app = test_app().await;

    let (status, body) = request_json(
        app,
        "POST",
        "/api/classes/999/students",
        json!({"name":"李四","age":11,"gender":"女"}),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "class not found");
}

#[tokio::test]
async fn invalid_class_payload_is_rejected() {
    let app = test_app().await;

    let (status, body) = request_json(
        app,
        "POST",
        "/api/classes",
        json!({"name":" ","teacher":"王老师","room":"301"}),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "class name, teacher, and room are required");
}
