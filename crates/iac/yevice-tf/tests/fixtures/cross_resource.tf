resource "aws_sqs_queue" "input_queue" {
  name = "input-queue"
}

resource "aws_dynamodb_table" "state_table" {
  name         = "state-table"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "id"

  attribute {
    name = "id"
    type = "S"
  }
}

# This lambda references state_table directly via a top-level attribute,
# simulating a dependency (e.g. IAM policy resource linking, or stream ARN).
resource "aws_lambda_function" "processor" {
  function_name    = "processor"
  runtime          = "python3.12"
  memory_size      = 256
  timeout          = 15
  handler          = "index.handler"
  filename         = "processor.zip"
  role             = "arn:aws:iam::123456789012:role/lambda-role"
  # Top-level ResourceRef: produces a DataFlow edge lambda → dynamodb
  reserved_concurrent_executions = aws_dynamodb_table.state_table.read_capacity
}

resource "aws_lambda_event_source_mapping" "sqs_trigger" {
  event_source_arn = aws_sqs_queue.input_queue.arn
  function_name    = aws_lambda_function.processor.arn
  batch_size       = 10
}
