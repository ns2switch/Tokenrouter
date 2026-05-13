import { useState, useEffect } from 'react';
import * as api from '../api/client';

const empty = { model_name: '', provider_id: 'default', provider_cost_input_per_1m: '', provider_cost_output_per_1m: '', bit_price_per_1m: '', node_price_per_1m: '', cluster_price_per_1m: '', min_tier: 'bit' };

export default function PricingManager() {
  const [pricing, setPricing] = useState([]);
  const [form, setForm] = useState(empty);
  const [error, setError] = useState('');
  const [providers, setProviders] = useState([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => { loadPricing(); loadProviders(); }, []);

  async function loadProviders() {
    try {
      const data = await api.getProviders();
      setProviders(data.providers || []);
    } catch { /* silent */ }
  }

  async function loadPricing() {
    setLoading(true);
    try {
      setPricing(await api.getPricing());
      setError('');
    } catch (err) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  }

  async function handleSubmit(e) {
    e.preventDefault();
    setError('');
    try {
      await api.upsertPricing(
        form.model_name,
        form.provider_id,
        parseFloat(form.provider_cost_input_per_1m),
        parseFloat(form.provider_cost_output_per_1m),
        parseFloat(form.bit_price_per_1m),
        parseFloat(form.node_price_per_1m),
        parseFloat(form.cluster_price_per_1m),
        form.min_tier,
      );
      setForm(empty);
      await loadPricing();
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleDelete(model) {
    if (!confirm(`Delete pricing for ${model}?`)) return;
    setError('');
    try {
      await api.deletePricing(model);
      await loadPricing();
    } catch (err) {
      setError(err.message);
    }
  }

  async function loadPricing() {
    try {
      setPricing(await api.getPricing());
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleSubmit(e) {
    e.preventDefault();
    try {
      await api.upsertPricing(
        form.model_name,
        form.provider_id,
        parseFloat(form.provider_cost_input_per_1m),
        parseFloat(form.provider_cost_output_per_1m),
        parseFloat(form.bit_price_per_1m),
        parseFloat(form.node_price_per_1m),
        parseFloat(form.cluster_price_per_1m),
        form.min_tier,
      );
      setForm(empty);
      loadPricing();
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleDelete(model) {
    if (!confirm(`Delete pricing for ${model}?`)) return;
    try {
      await api.deletePricing(model);
      loadPricing();
    } catch (err) {
      setError(err.message);
    }
  }

  function set(field) {
    return (e) => setForm({ ...form, [field]: e.target.value });
  }

  return (
    <div className="space-y-6">
      {error && <p className="text-red-600 bg-red-50 p-3 rounded">{error}</p>}
      {loading && <p className="text-slate-500 bg-slate-50 p-3 rounded animate-pulse">Loading...</p>}

      <form onSubmit={handleSubmit} className="bg-white rounded-xl shadow p-4">
        <h2 className="font-semibold mb-3">Add / Update Pricing</h2>
        <div className="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-9 gap-2">
          <input className="border rounded p-2" placeholder="Model" value={form.model_name} onChange={set('model_name')} required />
          <select className="border rounded p-2" value={form.provider_id} onChange={set('provider_id')}>
            <option value="default">default</option>
            {providers.map((p) => (
              <option key={p.id} value={p.id}>{p.id}{p.name ? ` (${p.name})` : ''}</option>
            ))}
          </select>
          <input className="border rounded p-2" type="number" step="0.0001" placeholder="Prov in/1M" value={form.provider_cost_input_per_1m} onChange={set('provider_cost_input_per_1m')} required />
          <input className="border rounded p-2" type="number" step="0.0001" placeholder="Prov out/1M" value={form.provider_cost_output_per_1m} onChange={set('provider_cost_output_per_1m')} required />
          <input className="border rounded p-2" type="number" step="0.0001" placeholder="Bit/1M" value={form.bit_price_per_1m} onChange={set('bit_price_per_1m')} required />
          <input className="border rounded p-2" type="number" step="0.0001" placeholder="Node/1M" value={form.node_price_per_1m} onChange={set('node_price_per_1m')} required />
          <input className="border rounded p-2" type="number" step="0.0001" placeholder="Cluster/1M" value={form.cluster_price_per_1m} onChange={set('cluster_price_per_1m')} required />
          <select className="border rounded p-2" value={form.min_tier} onChange={set('min_tier')}>
            <option value="bit">Bit</option>
            <option value="node">Node</option>
            <option value="cluster">Cluster</option>
          </select>
          <button className="bg-slate-900 text-white rounded p-2 font-semibold text-sm">Save</button>
        </div>
      </form>

      <div className="bg-white rounded-xl shadow overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="text-left border-b bg-slate-50">
              <th className="p-3">Model</th>
              <th className="p-3">Prov</th>
              <th className="p-3">In/1M</th>
              <th className="p-3">Out/1M</th>
              <th className="p-3">Bit</th>
              <th className="p-3">Node</th>
              <th className="p-3">Cluster</th>
              <th className="p-3">Min Tier</th>
              <th className="p-3">Actions</th>
            </tr>
          </thead>
          <tbody>
            {pricing.map((p) => (
              <tr key={p.model_name} className="border-b">
                <td className="p-3">{p.model_name}</td>
                <td className="p-3 font-mono text-xs">{p.provider_id}</td>
                <td className="p-3">{p.provider_cost_input_per_1m}</td>
                <td className="p-3">{p.provider_cost_output_per_1m}</td>
                <td className="p-3">{p.bit_price_per_1m}</td>
                <td className="p-3">{p.node_price_per_1m}</td>
                <td className="p-3">{p.cluster_price_per_1m}</td>
                <td className="p-3">{p.min_tier}</td>
                <td className="p-3">
                  <button onClick={() => handleDelete(p.model_name)} className="text-xs bg-red-50 hover:bg-red-100 text-red-700 rounded px-2 py-1">
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
