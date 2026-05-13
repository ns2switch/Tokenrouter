import { useState, useEffect } from 'react';
import * as api from '../api/client';

const empty = {
  id: '', name: '', base_url: '', api_key: '',
  quantization: 'fp16', context_length: '128000',
  max_output_length: '8192', datacenter_country: 'US',
};

export default function ProvidersManager() {
  const [providers, setProviders] = useState([]);
  const [form, setForm] = useState(empty);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  useEffect(() => { load(); }, []);

  async function load() {
    setLoading(true);
    try {
      const data = await api.getProviders();
      setProviders(data.providers || []);
      setError('');
    } catch (err) {
      setError(err.message);
    } finally {
      setLoading(false);
    }
  }

  async function handleSubmit(e) {
    e.preventDefault();
    if (!form.id || !form.base_url) {
      setError('id and base_url are required');
      return;
    }
    setError('');
    try {
      await api.upsertProvider(
        form.id, form.name, form.base_url, form.api_key,
        form.quantization, parseInt(form.context_length) || 128000,
        parseInt(form.max_output_length) || 8192, form.datacenter_country,
      );
      setForm(empty);
      await load();
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleEdit(p) {
    setForm({
      id: p.id,
      name: p.name || '',
      base_url: p.base_url || '',
      api_key: '',
      quantization: p.quantization || 'fp16',
      context_length: String(p.context_length || 128000),
      max_output_length: String(p.max_output_length || 8192),
      datacenter_country: p.datacenter_country || 'US',
    });
    setError('');
  }

  async function handleDelete(id) {
    if (!confirm(`Delete provider ${id}?`)) return;
    setError('');
    try {
      await api.deleteProvider(id);
      await load();
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleRefresh() {
    setError('');
    try {
      await api.refreshProviders();
      await load();
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

      <div className="bg-white rounded-xl shadow p-4 flex items-center justify-between">
        <h2 className="font-semibold">Providers ({providers.length})</h2>
        <button onClick={handleRefresh} className="bg-slate-100 hover:bg-slate-200 text-slate-700 rounded px-3 py-1 text-sm">
          Refresh Cache
        </button>
      </div>

      <form onSubmit={handleSubmit} className="bg-white rounded-xl shadow p-4">
        <h2 className="font-semibold mb-3">Add / Update Provider</h2>
        <div className="grid grid-cols-2 md:grid-cols-4 gap-2">
          <input className="border rounded p-2" placeholder="ID" value={form.id} onChange={set('id')} required />
          <input className="border rounded p-2" placeholder="Name" value={form.name} onChange={set('name')} />
          <input className="border rounded p-2 col-span-2" placeholder="Base URL" value={form.base_url} onChange={set('base_url')} required />
          <input className="border rounded p-2 col-span-2" type="password" placeholder="API Key (leave blank to keep current)" value={form.api_key} onChange={set('api_key')} />
          <input className="border rounded p-2" placeholder="Quantization" value={form.quantization} onChange={set('quantization')} />
          <input className="border rounded p-2" type="number" placeholder="Context Length" value={form.context_length} onChange={set('context_length')} />
          <input className="border rounded p-2" type="number" placeholder="Max Output" value={form.max_output_length} onChange={set('max_output_length')} />
          <input className="border rounded p-2" placeholder="Datacenter" value={form.datacenter_country} onChange={set('datacenter_country')} />
          <button className="bg-slate-900 text-white rounded p-2 font-semibold text-sm col-span-2 md:col-span-1">Save</button>
        </div>
      </form>

      <div className="bg-white rounded-xl shadow overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="text-left border-b bg-slate-50">
              <th className="p-3">ID</th>
              <th className="p-3">Name</th>
              <th className="p-3">Base URL</th>
              <th className="p-3">Quant</th>
              <th className="p-3">Context</th>
              <th className="p-3">Max Out</th>
              <th className="p-3">DC</th>
              <th className="p-3">Actions</th>
            </tr>
          </thead>
          <tbody>
            {providers.map((p) => (
              <tr key={p.id} className="border-b">
                <td className="p-3 font-mono text-xs">{p.id}</td>
                <td className="p-3">{p.name}</td>
                <td className="p-3 font-mono text-xs max-w-48 truncate">{p.base_url}</td>
                <td className="p-3">{p.quantization}</td>
                <td className="p-3">{p.context_length}</td>
                <td className="p-3">{p.max_output_length}</td>
                <td className="p-3">{p.datacenter_country}</td>
                <td className="p-3 flex gap-1">
                  <button onClick={() => handleEdit(p)} className="text-xs bg-slate-100 hover:bg-slate-200 text-slate-700 rounded px-2 py-1">
                    Edit
                  </button>
                  <button onClick={() => handleDelete(p.id)} className="text-xs bg-red-50 hover:bg-red-100 text-red-700 rounded px-2 py-1">
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
