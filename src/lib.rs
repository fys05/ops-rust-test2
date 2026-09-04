use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Row, SqlitePool};
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};

#[derive(Clone)]
pub struct AppState {
    pool: SqlitePool,
}

#[derive(Debug, Serialize)]
pub struct Class {
    pub id: i64,
    pub name: String,
    pub teacher: String,
    pub room: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateClass {
    pub name: String,
    pub teacher: String,
    pub room: String,
}

#[derive(Debug, Serialize)]
pub struct Student {
    pub id: i64,
    pub class_id: i64,
    pub name: String,
    pub age: i64,
    pub gender: String,
    pub phone: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateStudent {
    pub name: String,
    pub age: i64,
    pub gender: String,
    pub phone: Option<String>,
}

#[derive(Debug)]
pub enum ApiError {
    BadRequest(&'static str),
    NotFound(&'static str),
    Database(sqlx::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message),
            Self::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "database error"),
        };

        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub fn app(pool: SqlitePool) -> Router {
    let state = AppState { pool };

    Router::new()
        .route("/api/health", get(health))
        .route("/api/classes", get(list_classes).post(create_class))
        .route(
            "/api/classes/:class_id/students",
            get(list_students).post(create_student),
        )
        .nest_service("/", ServeDir::new("public"))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn init_db(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS classes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            teacher TEXT NOT NULL,
            room TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS students (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            class_id INTEGER NOT NULL,
            name TEXT NOT NULL,
            age INTEGER NOT NULL,
            gender TEXT NOT NULL,
            phone TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (class_id) REFERENCES classes(id) ON DELETE CASCADE
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn list_classes(State(state): State<AppState>) -> Result<Json<Vec<Class>>, ApiError> {
    let rows = sqlx::query(
        r#"
        SELECT id, name, teacher, room
        FROM classes
        ORDER BY id DESC
        "#,
    )
    .fetch_all(&state.pool)
    .await?;

    let classes = rows
        .into_iter()
        .map(|row| Class {
            id: row.get("id"),
            name: row.get("name"),
            teacher: row.get("teacher"),
            room: row.get("room"),
        })
        .collect();

    Ok(Json(classes))
}

async fn create_class(
    State(state): State<AppState>,
    Json(payload): Json<CreateClass>,
) -> Result<impl IntoResponse, ApiError> {
    let name = payload.name.trim();
    let teacher = payload.teacher.trim();
    let room = payload.room.trim();

    if name.is_empty() || teacher.is_empty() || room.is_empty() {
        return Err(ApiError::BadRequest(
            "class name, teacher, and room are required",
        ));
    }

    let result = sqlx::query("INSERT INTO classes (name, teacher, room) VALUES (?, ?, ?)")
        .bind(name)
        .bind(teacher)
        .bind(room)
        .execute(&state.pool)
        .await?;

    let class = Class {
        id: result.last_insert_rowid(),
        name: name.to_owned(),
        teacher: teacher.to_owned(),
        room: room.to_owned(),
    };

    Ok((StatusCode::CREATED, Json(class)))
}

async fn list_students(
    State(state): State<AppState>,
    Path(class_id): Path<i64>,
) -> Result<Json<Vec<Student>>, ApiError> {
    ensure_class_exists(&state.pool, class_id).await?;

    let rows = sqlx::query(
        r#"
        SELECT id, class_id, name, age, gender, phone
        FROM students
        WHERE class_id = ?
        ORDER BY id DESC
        "#,
    )
    .bind(class_id)
    .fetch_all(&state.pool)
    .await?;

    let students = rows
        .into_iter()
        .map(|row| Student {
            id: row.get("id"),
            class_id: row.get("class_id"),
            name: row.get("name"),
            age: row.get("age"),
            gender: row.get("gender"),
            phone: row.get("phone"),
        })
        .collect();

    Ok(Json(students))
}

async fn create_student(
    State(state): State<AppState>,
    Path(class_id): Path<i64>,
    Json(payload): Json<CreateStudent>,
) -> Result<impl IntoResponse, ApiError> {
    ensure_class_exists(&state.pool, class_id).await?;

    let name = payload.name.trim();
    let gender = payload.gender.trim();
    let phone = payload
        .phone
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);

    if name.is_empty() || gender.is_empty() || payload.age <= 0 {
        return Err(ApiError::BadRequest(
            "student name, positive age, and gender are required",
        ));
    }

    let result = sqlx::query(
        "INSERT INTO students (class_id, name, age, gender, phone) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(class_id)
    .bind(name)
    .bind(payload.age)
    .bind(gender)
    .bind(&phone)
    .execute(&state.pool)
    .await?;

    let student = Student {
        id: result.last_insert_rowid(),
        class_id,
        name: name.to_owned(),
        age: payload.age,
        gender: gender.to_owned(),
        phone,
    };

    Ok((StatusCode::CREATED, Json(student)))
}

async fn ensure_class_exists(pool: &SqlitePool, class_id: i64) -> Result<(), ApiError> {
    let exists: Option<i64> = sqlx::query_scalar("SELECT id FROM classes WHERE id = ?")
        .bind(class_id)
        .fetch_optional(pool)
        .await?;

    exists
        .map(|_| ())
        .ok_or(ApiError::NotFound("class not found"))
}
