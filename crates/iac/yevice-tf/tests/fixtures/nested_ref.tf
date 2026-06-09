resource "aws_dynamodb_table" "my_table" {
  name         = "my-table"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "id"

  attribute {
    name = "id"
    type = "S"
  }
}

resource "aws_lambda_function" "my_function" {
  function_name = "my-function"
  runtime       = "python3.12"
  memory_size   = 256
  timeout       = 15
  handler       = "index.handler"
  filename      = "function.zip"
  role          = "arn:aws:iam::123456789012:role/lambda-role"

  environment {
    variables = {
      TABLE_ARN  = aws_dynamodb_table.my_table.arn
      TABLE_NAME = "my-table"
    }
  }
}

resource "aws_sqs_queue" "my_queue" {
  name = "my-queue"
}

resource "aws_lambda_function" "list_fn" {
  function_name = "list-fn"
  runtime       = "python3.12"
  memory_size   = 128
  timeout       = 10
  handler       = "index.handler"
  filename      = "list_fn.zip"
  role          = "arn:aws:iam::123456789012:role/lambda-role"

  environment {
    variables = {
      QUEUE_URLS = [aws_sqs_queue.my_queue.url]
    }
  }
}
