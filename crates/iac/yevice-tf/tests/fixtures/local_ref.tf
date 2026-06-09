resource "aws_lambda_function" "fn" {
  function_name = "my-function"
  runtime       = "python3.12"
  memory_size   = 256
  timeout       = 15
  handler       = "index.handler"
  filename      = "fn.zip"
  role          = "arn:aws:iam::123456789012:role/lambda-role"
}

resource "aws_s3_bucket" "uploads" {
  bucket = "uploads-bucket"
}

locals {
  # Alias for the Lambda ARN — stored as a ResourceRef
  fn_arn = aws_lambda_function.fn.arn
}

resource "aws_s3_bucket_notification" "upload_notif" {
  bucket = aws_s3_bucket.uploads.id

  lambda_function {
    lambda_function_arn = local.fn_arn
    events              = ["s3:ObjectCreated:*"]
  }
}
