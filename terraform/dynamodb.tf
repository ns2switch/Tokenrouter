resource "aws_dynamodb_table" "api_keys" {
  name           = "${var.app_name}_api_keys_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "key_hash"

  attribute {
    name = "key_hash"
    type = "S"
  }

  tags = { Name = "${var.app_name}-api-keys" }
}

resource "aws_dynamodb_table" "pricing_config" {
  name           = "${var.app_name}_pricing_config_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "model_name"

  attribute {
    name = "model_name"
    type = "S"
  }

  tags = { Name = "${var.app_name}-pricing-config" }
}

resource "aws_dynamodb_table" "transactions" {
  name           = "${var.app_name}_transactions_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "id"

  attribute {
    name = "id"
    type = "S"
  }

  tags = { Name = "${var.app_name}-transactions" }
}

resource "aws_dynamodb_table" "transactions_timeline" {
  name           = "${var.app_name}_transactions_timeline_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "timeline_pk"
  range_key      = "timeline_sk"

  attribute {
    name = "timeline_pk"
    type = "S"
  }

  attribute {
    name = "timeline_sk"
    type = "S"
  }

  tags = { Name = "${var.app_name}-transactions-timeline" }
}

resource "aws_dynamodb_table" "idempotency_keys" {
  name           = "${var.app_name}_idempotency_keys_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "idempotency_key"

  attribute {
    name = "idempotency_key"
    type = "S"
  }

  ttl {
    enabled        = true
    attribute_name = "expires_at"
  }

  tags = { Name = "${var.app_name}-idempotency-keys" }
}

resource "aws_dynamodb_table" "metrics_global" {
  name           = "${var.app_name}_metrics_global_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "metric_id"

  attribute {
    name = "metric_id"
    type = "S"
  }

  tags = { Name = "${var.app_name}-metrics-global" }
}

resource "aws_dynamodb_table" "providers" {
  name           = "${var.app_name}_providers_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "id"

  attribute {
    name = "id"
    type = "S"
  }

  tags = { Name = "${var.app_name}-providers" }
}

resource "aws_dynamodb_table" "dead_letter" {
  name           = "${var.app_name}_dead_letter_${var.environment}"
  billing_mode   = "PAY_PER_REQUEST"
  hash_key       = "id"

  attribute {
    name = "id"
    type = "S"
  }

  tags = { Name = "${var.app_name}-dead-letter" }
}
