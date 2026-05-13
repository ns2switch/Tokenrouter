resource "aws_ecr_repository" "app" {
  name         = "${var.app_name}-${var.environment}"
  force_delete = true
}

resource "aws_lambda_function" "app" {
  function_name = "${var.app_name}-${var.environment}"
  role          = aws_iam_role.lambda.arn
  package_type  = "Image"
  image_uri     = "${aws_ecr_repository.app.repository_url}:latest"
  timeout       = 300
  memory_size   = 1024

  environment {
    variables = {
      RUN_MODE                       = "lambda"
      NIM_BASE_URL                   = var.nim_base_url
      NIM_API_KEY                    = var.nim_api_key
      API_KEY_HASH_SECRET            = var.api_key_hash_secret
      ADMIN_BEARER_TOKENS            = var.admin_bearer_tokens
      ADMIN_IP_ALLOWLIST             = var.admin_ip_allowlist
      MAX_INFLIGHT_REQUESTS          = tostring(var.max_inflight_requests)
      IDEMPOTENCY_TTL_SECONDS        = tostring(var.idempotency_ttl_seconds)
      TIMELINE_SHARD_COUNT           = tostring(var.timeline_shard_count)
      METRICS_FLUSH_INTERVAL_SECONDS = tostring(var.metrics_flush_interval_seconds)
      PROVIDER_NAME_PREFIX           = var.provider_name_prefix
      PROVIDER_QUANTIZATION          = var.provider_quantization
      PROVIDER_CONTEXT_LENGTH        = tostring(var.provider_context_length)
      PROVIDER_MAX_OUTPUT_LENGTH     = tostring(var.provider_max_output_length)
      PROVIDER_DATACENTER_COUNTRY    = var.provider_datacenter_country
      UPSTREAM_TIMEOUT_SECONDS       = tostring(var.upstream_timeout_seconds)
      MAX_INFLIGHT_PER_KEY           = tostring(var.max_inflight_per_key)
      MAX_OUTPUT_TOKENS              = tostring(var.max_output_tokens)
      MAX_STREAMING_DURATION_SECONDS = tostring(var.max_streaming_seconds)
      REQUEST_CACHE_TTL_SECONDS      = tostring(var.request_cache_ttl_seconds)
      REQUEST_CACHE_MAX_ENTRIES      = tostring(var.request_cache_max_entries)
      REQUEST_CACHE_MAX_RESPONSE_BYTES = tostring(var.request_cache_max_response_bytes)
      DDB_API_KEYS_TABLE             = aws_dynamodb_table.api_keys.name
      DDB_PROVIDERS_TABLE           = aws_dynamodb_table.providers.name
      DDB_PRICING_TABLE              = aws_dynamodb_table.pricing_config.name
      DDB_TRANSACTIONS_TABLE         = aws_dynamodb_table.transactions.name
      DDB_TRANSACTIONS_TIMELINE_TABLE = aws_dynamodb_table.transactions_timeline.name
      DDB_IDEMPOTENCY_TABLE          = aws_dynamodb_table.idempotency_keys.name
      DDB_METRICS_TABLE              = aws_dynamodb_table.metrics_global.name
      DDB_DEAD_LETTER_TABLE          = aws_dynamodb_table.dead_letter.name
      RUST_LOG                       = "info"
    }
  }
}

# API Gateway HTTP API (replaces public Function URL)
resource "aws_apigatewayv2_api" "app" {
  name          = "${var.app_name}-${var.environment}"
  protocol_type = "HTTP"
}

resource "aws_apigatewayv2_integration" "app" {
  api_id             = aws_apigatewayv2_api.app.id
  integration_type   = "AWS_PROXY"
  integration_uri    = aws_lambda_function.app.invoke_arn
  payload_format_version = "2.0"
}

resource "aws_lambda_permission" "apigw" {
  statement_id  = "AllowAPIGatewayInvoke"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.app.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.app.execution_arn}/*/*"
}

resource "aws_apigatewayv2_route" "proxy" {
  api_id    = aws_apigatewayv2_api.app.id
  route_key = "$default"
  target    = "integrations/${aws_apigatewayv2_integration.app.id}"
}

resource "aws_apigatewayv2_stage" "default" {
  api_id      = aws_apigatewayv2_api.app.id
  name        = "$default"
  auto_deploy = true
}

resource "aws_cloudwatch_log_group" "app" {
  name              = "/aws/lambda/${aws_lambda_function.app.function_name}"
  retention_in_days = 30
}



