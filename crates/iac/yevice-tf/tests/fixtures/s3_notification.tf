resource "aws_s3_bucket" "my_bucket" {
}

resource "aws_lambda_function" "my_lambda" {
  function_name = "my-function"
  role          = "arn:aws:iam::123456789012:role/role"
  runtime       = "python3.12"
  handler       = "index.handler"
  memory_size   = 128
  timeout       = 3
}

resource "aws_s3_bucket_notification" "bucket_notif" {
  bucket = aws_s3_bucket.my_bucket.id

  lambda_function {
    lambda_function_arn = aws_lambda_function.my_lambda.arn
    events              = ["s3:ObjectCreated:*"]
  }
}
