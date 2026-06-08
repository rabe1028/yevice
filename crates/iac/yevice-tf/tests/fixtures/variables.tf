variable "memory_size" {
  default = 256
}

variable "instance_type" {
  default = "t3.micro"
}

resource "aws_lambda_function" "handler" {
  function_name = "var-lambda"
  memory_size   = var.memory_size
  timeout       = 10
  runtime       = "nodejs20.x"
  filename      = "handler.zip"
  handler       = "index.handler"
  role          = "arn:aws:iam::123456789012:role/lambda-role"
}

resource "aws_instance" "server" {
  ami           = "ami-12345"
  instance_type = var.instance_type
}
