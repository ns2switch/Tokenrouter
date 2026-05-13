import { useState, useEffect } from 'react';
import * as api from '../api/client';

export default function KeysManager() {
  const [keys, setKeys] = useState([]);
  const [error, setError] = useState('');
  const [tier, setTier] = useState('bit');
  const [creditLimit, setCreditLimit] = useState('');
  const [newKey, setNewKey] = useState(null);

  useEffect(() => { loadKeys(); }, []);

  async function loadKeys() {
    try {
      setKeys(await api.getKeys());
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleCreate(e) {
    e.preventDefault();
    try {
      const result = await api.createKey(tier, creditLimit ? parseFloat(creditLimit) : null);
      setNewKey(result);
      setCreditLimit('');
      loadKeys();
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleToggle(id, active) {
    try {
      await api.toggleKey(id, !active);
      loadKeys();
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleDelete(id) {
    if (!confirm('Delete this key?')) return;
    try {
      await api.deleteKey(id);
      loadKeys();
    } catch (err) {
      setError(err.message);
    }
  }

  async function handleLimitUpdate(id, currentLimit) {
    const val = prompt('Credit limit (empty to remove):', currentLimit || '');
    if (val === null) return;
    try {
      await api.updateKeyLimit(id, val !== '' ? val : undefined);
      loadKeys();
    } catch (err) {
      setError(err.message);
    }
  }

  return (
    <div className="space-y-6">
      {error && <p className="text-red-600 bg-red-50 p-3 rounded">{error}</p>}
      {newKey && (
        <div className="bg-green-50 border border-green-300 rounded-xl p-4">
          <p className="font-bold text-green-800">Key created — save it now!</p>
          <code className="text-sm break-all">{newKey.raw_key}</code>
          <p className="text-xs text-green-600 mt-1">ID: {newKey.id} | Tier: {newKey.tier}</p>
          <button onClick={() => setNewKey(null)} className="text-xs underline mt-2">Dismiss</button>
        </div>
      )}

      <form onSubmit={handleCreate} className="bg-white rounded-xl shadow p-4 flex gap-2 items-end">
        <div>
          <label className="text-xs text-slate-500 block">Tier</label>
          <select value={tier} onChange={(e) => setTier(e.target.value)} className="border rounded p-2">
            <option value="bit">bit</option>
            <option value="node">node</option>
            <option value="cluster">cluster</option>
          </select>
        </div>
        <div>
          <label className="text-xs text-slate-500 block">Credit Limit</label>
          <input
            type="number" step="0.01" className="border rounded p-2 w-32"
            value={creditLimit} onChange={(e) => setCreditLimit(e.target.value)}
            placeholder="Optional"
          />
        </div>
        <button className="bg-slate-900 text-white rounded px-4 py-2 text-sm font-semibold">
          Create Key
        </button>
      </form>

      <div className="bg-white rounded-xl shadow overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="text-left border-b bg-slate-50">
              <th className="p-3">ID</th>
              <th className="p-3">Tier</th>
              <th className="p-3">Balance</th>
              <th className="p-3">Credit Limit</th>
              <th className="p-3">Active</th>
              <th className="p-3">Created</th>
              <th className="p-3">Actions</th>
            </tr>
          </thead>
          <tbody>
            {keys.map((k) => (
              <tr key={k.id} className="border-b">
                <td className="p-3 font-mono text-xs">{k.id.slice(0, 8)}...</td>
                <td className="p-3">{k.tier}</td>
                <td className="p-3">${k.balance_accumulated.toFixed(4)}</td>
                <td className="p-3">{k.credit_limit != null ? `$${k.credit_limit}` : '—'}</td>
                <td className="p-3">
                  <span className={`px-2 py-0.5 rounded text-xs ${k.active ? 'bg-green-100 text-green-800' : 'bg-red-100 text-red-800'}`}>
                    {k.active ? 'yes' : 'no'}
                  </span>
                </td>
                <td className="p-3 text-xs">{new Date(k.created_at).toLocaleDateString()}</td>
                <td className="p-3 flex gap-1">
                  <button onClick={() => handleToggle(k.id, k.active)} className="text-xs bg-slate-100 hover:bg-slate-200 rounded px-2 py-1">
                    {k.active ? 'Disable' : 'Enable'}
                  </button>
                  <button onClick={() => handleLimitUpdate(k.id, k.credit_limit)} className="text-xs bg-slate-100 hover:bg-slate-200 rounded px-2 py-1">
                    Limit
                  </button>
                  <button onClick={() => handleDelete(k.id)} className="text-xs bg-red-50 hover:bg-red-100 text-red-700 rounded px-2 py-1">
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
