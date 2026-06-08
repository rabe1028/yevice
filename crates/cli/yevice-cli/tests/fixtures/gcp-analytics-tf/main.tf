variable "bucket_name" {
  default = "analytics-events"
}

variable "bucket_location" {
  default = "ASIA-NORTHEAST1"
}

resource "google_storage_bucket" "artifacts" {
  name          = var.bucket_name
  location      = var.bucket_location
  storage_class = "STANDARD"
}

resource "google_pubsub_topic" "events" {
  name = "analytics-events"
}
