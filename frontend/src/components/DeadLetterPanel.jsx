import { useState, useEffect } from 'react';
import * as api from '../api/client';

export default function DeadLetterPanel() {
  const [data, setData] = useState(null);
  const [error, setError] = useState('');

  useEffect(() => { load(); }, []);

  async function load() {
    try {
      setData(await api.getDeadLetters());
    } catch (err) {
      setError(err.message);
    }
  }

  return (
    <div className="space-y-6">
      {error && <p className="text-red-600 bg-red-50 p-3 rounded">{error}</p>}

      <div className="bg-white rounded-xl shadow p-4 flex items-center justify-between">
        <h2 className="font-semibold">
          Dead Letter Entries ({data ? data.count : '...'})
        </h2>
        <button onClick={load} className="bg-slate-100 hover:bg-slate-200 text-slate-700 rounded px-3 py-1 text-sm">
          Refresh
        </button>
      </div>

      {data && data.entries && data.entries.length === 0 && (
        <div className="bg-white rounded-xl shadow p-8 text-center text-slate-400">
          No dead letter entries found.
        </div>
      )}

      {data && data.entries && data.entries.length > 0 && (
        <div className="bg-white rounded-xl shadow overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="text-left border-b bg-slate-50">
                <th className="p-3">ID</th>
                <th className="p-3">Key</th>
                <th className="p-3">Model</th>
                <th className="p-3">In</th>
                <th className="p-3">Out</th>
                <th className="p-3">Cost</th>
                <th className="p-3">Revenue</th>
                <th className="p-3">Timestamp</th>
                <th className="p-3">Error</th>
              </tr>
            </thead>
            <tbody>
              {data.entries.map((e) => (
                <tr key={e.id} className="border-b">
                  <td className="p-3 font-mono text-xs">{e.id}</td>
                  <td className="p-3 font-mono text-xs">{e.api_key_id}</td>
                  <td className="p-3">{e.model_name}</td>
                  <td className="p-3">{e.input_tokens}</td>
                  <td className="p-3">{e.output_tokens}</td>
                  <td className="p-3">${e.cost_basis.toFixed(4)}</td>
                  <td className="p-3">${e.revenue_generated.toFixed(4)}</td>
                  <td className="p-3 text-xs">{e.timestamp}</td>
                  <td className="p-3 max-w-64 truncate text-xs text-red-600">{e.error}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
