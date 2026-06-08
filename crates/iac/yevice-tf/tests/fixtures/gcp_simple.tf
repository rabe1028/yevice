resource "google_cloudfunctions2_function" "api" {
  name     = "my-api"
  location = "asia-northeast1"

  service_config {
    available_memory   = "512Mi"
    min_instance_count = 0
  }
}

resource "google_storage_bucket" "data" {
  name          = "my-data"
  location      = "asia-northeast1"
  storage_class = "STANDARD"
}

resource "google_sql_database_instance" "db" {
  name             = "my-db"
  database_version = "POSTGRES_15"
  region           = "asia-northeast1"

  settings {
    tier              = "db-n1-standard-2"
    availability_type = "REGIONAL"
    disk_size         = 100
  }
}

resource "google_pubsub_topic" "events" {
  name = "my-events"
}
