variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "us-east-1"
}

variable "app_name" {
  description = "Application name"
  type        = string
  default     = "tokenrouter"
}

variable "environment" {
  description = "Deployment environment"
  type        = string
  default     = "production"
}

variable "nim_base_url" {
  description = "NVIDIA NIM base URL"
  type        = string
  sensitive   = true
}

variable "nim_api_key" {
  description = "NVIDIA NIM API key"
  type        = string
  sensitive   = true
}

variable "api_key_hash_secret" {
  description = "Secret for API key hashing"
  type        = string
  sensitive   = true
}

variable "admin_bearer_tokens" {
  description = "CSV of admin bearer tokens"
  type        = string
  sensitive   = true
}

variable "admin_ip_allowlist" {
  description = "Comma-separated CIDR allowlist for admin access"
  type        = string
  default     = ""
}

variable "max_inflight_requests" {
  description = "Maximum concurrent requests"
  type        = number
  default     = 200
}

variable "idempotency_ttl_seconds" {
  description = "TTL for idempotency keys in seconds"
  type        = number
  default     = 86400
}

variable "timeline_shard_count" {
  description = "Number of shards for transactions timeline"
  type        = number
  default     = 1
}

variable "metrics_flush_interval_seconds" {
  description = "Interval for flushing runtime metrics to DynamoDB"
  type        = number
  default     = 300
}

variable "cors_allow_origin" {
  description = "CORS allowed origin for Lambda Function URL"
  type        = string
  default     = "*"
}

variable "alarm_email" {
  description = "Email address for CloudWatch alarm notifications"
  type        = string
  default     = "alerts@example.com"
}

variable "provider_name_prefix" {
  description = "Provider name prefix for model lists"
  type        = string
  default     = "TokenRouter"
}

variable "provider_quantization" {
  description = "Provider quantization metadata"
  type        = string
  default     = "fp16"
}

variable "provider_context_length" {
  description = "Provider context length metadata"
  type        = number
  default     = 128000
}

variable "provider_max_output_length" {
  description = "Provider max output length metadata"
  type        = number
  default     = 8192
}

variable "provider_datacenter_country" {
  description = "Provider datacenter country code"
  type        = string
  default     = "US"
}

variable "upstream_timeout_seconds" {
  description = "HTTP client timeout for upstream calls"
  type        = number
  default     = 300
}

variable "max_inflight_per_key" {
  description = "Maximum concurrent requests per API key"
  type        = number
  default     = 10
}

variable "max_output_tokens" {
  description = "Cap for max_tokens in forwarded requests"
  type        = number
  default     = 16384
}

variable "max_streaming_seconds" {
  description = "Maximum duration for streaming requests"
  type        = number
  default     = 600
}

variable "request_cache_ttl_seconds" {
  description = "TTL for cached responses in seconds"
  type        = number
  default     = 3600
}

variable "request_cache_max_entries" {
  description = "Maximum number of cached entries"
  type        = number
  default     = 1000
}

variable "request_cache_max_response_bytes" {
  description = "Maximum bytes per cached response"
  type        = number
  default     = 65536
}
