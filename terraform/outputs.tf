output "lambda_function_name" {
  value       = aws_lambda_function.app.function_name
  description = "Lambda function name"
}

output "api_gateway_url" {
  value       = aws_apigatewayv2_stage.default.invoke_url
  description = "API Gateway invoke URL"
}

output "ecr_repository_url" {
  value       = aws_ecr_repository.app.repository_url
  description = "ECR repository URL for container image"
}
