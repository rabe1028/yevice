resource "aws_s3_bucket" "storage" {
  bucket = "my-storage"

  versioning {
    enabled = true
  }
}

resource "aws_kinesis_stream" "events" {
  name             = "events"
  shard_count      = 4
  retention_period = 48
}

resource "aws_ecs_service" "api" {
  name          = "api-service"
  desired_count = 3
  launch_type   = "FARGATE"
}
