resource "aws_iam_role" "lambda_exec" {
  name               = "lambda-exec-role"
  assume_role_policy = "{}"
}

resource "aws_cloudwatch_log_group" "lambda_logs" {
  name              = "/aws/lambda/processor"
  retention_in_days = 7
}

resource "aws_s3_bucket" "uploads" {
  bucket = "uploads-bucket"
}

resource "aws_dynamodb_table" "items" {
  name         = "items"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "id"

  attribute {
    name = "id"
    type = "S"
  }
}

resource "aws_lambda_function" "processor" {
  function_name = "processor"
  runtime       = "python3.12"
  memory_size   = 256
  timeout       = 15
  handler       = "index.handler"
  filename      = "processor.zip"
  # IAM role reference — must NOT produce an edge
  role          = aws_iam_role.lambda_exec.arn
}

resource "aws_lambda_function" "writer" {
  function_name = "writer"
  runtime       = "python3.12"
  memory_size   = 128
  timeout       = 10
  handler       = "index.handler"
  filename      = "writer.zip"
  role          = aws_iam_role.lambda_exec.arn

  environment {
    variables = {
      BUCKET_NAME = aws_s3_bucket.uploads.id
      TABLE_NAME  = aws_dynamodb_table.items.name
    }
  }
}
