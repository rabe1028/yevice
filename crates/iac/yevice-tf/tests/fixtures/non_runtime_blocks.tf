resource "aws_sqs_queue" "dlq" {
  name = "my-dlq"
}

resource "aws_dynamodb_table" "data_table" {
  name         = "data-table"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "id"

  server_side_encryption {
    enabled     = true
    kms_key_arn = "arn:aws:kms:us-east-1:123456789012:key/00000000-0000-0000-0000-000000000000"
  }

  attribute {
    name = "id"
    type = "S"
  }
}

resource "aws_lambda_function" "my_fn" {
  function_name = "my-fn"
  runtime       = "python3.12"
  memory_size   = 256
  timeout       = 15
  handler       = "index.handler"
  filename      = "my_fn.zip"
  role          = "arn:aws:iam::123456789012:role/lambda-role"
  kms_key_arn   = "arn:aws:kms:us-east-1:123456789012:key/00000000-0000-0000-0000-000000000000"

  # Non-runtime block: dead_letter_config must NOT produce a DataFlow edge.
  dead_letter_config {
    target_arn = aws_sqs_queue.dlq.arn
  }

  # Non-runtime block: X-Ray tracing config (also denylisted, no edge).
  tracing_config {
    mode = "Active"
  }

  # Runtime block: environment variables MUST still produce a DataFlow edge.
  environment {
    variables = {
      TABLE_ARN = aws_dynamodb_table.data_table.arn
    }
  }
}
