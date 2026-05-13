const BASE = '';

async function request(path, options = {}) {
  const res = await fetch(`${BASE}${path}`, {
    credentials: 'include',
    headers: { 'Content-Type': 'application/json', ...options.headers },
    ...options,
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error || res.statusText);
  }
  const text = await res.text();
  return text ? JSON.parse(text) : null;
}

export async function login(token) {
  return request('/admin/login', {
    method: 'POST',
    body: JSON.stringify({ token }),
  });
}

export async function logout() {
  return request('/admin/logout');
}

export async function getHealth() {
  return request('/admin/health');
}

export async function getDashboard() {
  return request('/admin/dashboard');
}

export async function getMetrics() {
  return request('/admin/metrics');
}

export async function getKeys() {
  return request('/admin/keys');
}

export async function createKey(tier, creditLimit) {
  return request('/admin/keys', {
    method: 'POST',
    body: JSON.stringify({ tier, credit_limit: creditLimit || null }),
  });
}

export async function toggleKey(id, active) {
  return request(`/admin/keys/toggle`, {
    method: 'POST',
    body: JSON.stringify({ id, active }),
  });
}

export async function updateKeyLimit(id, creditLimit) {
  const cl = creditLimit !== undefined ? String(creditLimit) : '';
  return request(`/admin/keys/limit`, {
    method: 'POST',
    body: JSON.stringify({ id, credit_limit: cl }),
  });
}

export async function deleteKey(id) {
  return request(`/admin/keys/delete`, {
    method: 'POST',
    body: JSON.stringify({ id }),
  });
}

export async function getPricing() {
  return request('/admin/pricing');
}

export async function upsertPricing(modelName, providerId, providerIn, providerOut, bit, node, cluster, minTier) {
  return request('/admin/pricing', {
    method: 'POST',
    body: JSON.stringify({
      model_name: modelName,
      provider_id: providerId || 'default',
      provider_cost_input_per_1m: providerIn,
      provider_cost_output_per_1m: providerOut,
      bit_price_per_1m: bit,
      node_price_per_1m: node,
      cluster_price_per_1m: cluster,
      min_tier: minTier || 'bit',
    }),
  });
}

export async function deletePricing(model) {
  return request(`/admin/pricing/delete`, {
    method: 'POST',
    body: JSON.stringify({ model }),
  });
}

export async function getTransactions(cursor, prev) {
  const params = new URLSearchParams();
  if (cursor) params.set('cursor', cursor);
  if (prev) params.set('prev', prev);
  const qs = params.toString();
  return request(`/admin/transactions${qs ? `?${qs}` : ''}`);
}

export async function getProviders() {
  return request('/admin/providers');
}

export async function upsertProvider(id, name, baseUrl, apiKey, quantization, contextLength, maxOutputLength, datacenterCountry) {
  return request('/admin/providers', {
    method: 'POST',
    body: JSON.stringify({
      id,
      name,
      base_url: baseUrl,
      api_key: apiKey,
      quantization: quantization || 'fp16',
      context_length: contextLength || 128000,
      max_output_length: maxOutputLength || 8192,
      datacenter_country: datacenterCountry || 'US',
    }),
  });
}

export async function deleteProvider(id) {
  return request('/admin/providers/delete', {
    method: 'POST',
    body: JSON.stringify({ id }),
  });
}

export async function refreshProviders() {
  return request('/admin/providers/refresh', { method: 'POST' });
}

export async function getDeadLetters() {
  return request('/admin/dead-letter');
}

export async function getUpstreamModels() {
  return request('/admin/models/upstream');
}

export async function getCacheStats() {
  return request('/admin/cache');
}
