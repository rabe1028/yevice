resource "aws_lambda_function" "handler" {
  function_name = "my-handler"
  runtime       = "python3.12"
  memory_size   = 512
  timeout       = 30
  handler       = "index.handler"
  filename      = "handler.zip"
  role          = "arn:aws:iam::123456789012:role/lambda-role"
}

resource "aws_sqs_queue" "queue" {
  name       = "my-queue"
  fifo_queue = false
}

resource "aws_dynamodb_table" "data" {
  name         = "my-table"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "id"

  attribute {
    name = "id"
    type = "S"
  }
}
