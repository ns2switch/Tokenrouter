import { useState, useEffect } from 'react';
import * as api from '../api/client';

export default function Transactions() {
  const [data, setData] = useState(null);
  const [error, setError] = useState('');
  const [cursor, setCursor] = useState(null);
  const [prev, setPrev] = useState(null);

  useEffect(() => { load(); }, [cursor, prev]);

  async function load() {
    try {
      const result = await api.getTransactions(cursor, prev);
      setData(result);
    } catch (err) {
      setError(err.message);
    }
  }

  if (error) return <p className="text-red-600">{error}</p>;
  if (!data) return <p>Loading...</p>;

  return (
    <div className="space-y-4">
      <div className="bg-white rounded-xl shadow overflow-x-auto">
        <table className="w-full text-sm">
          <thead>
            <tr className="text-left border-b bg-slate-50">
              <th className="p-3">Timestamp</th>
              <th className="p-3">Model</th>
              <th className="p-3">Key ID</th>
              <th className="p-3">In</th>
              <th className="p-3">Out</th>
              <th className="p-3">Cost</th>
              <th className="p-3">Revenue</th>
            </tr>
          </thead>
          <tbody>
            {data.transactions.length === 0 && (
              <tr><td colSpan={7} className="p-6 text-center text-slate-400">No transactions yet</td></tr>
            )}
            {data.transactions.map((tx) => (
              <tr key={tx.id} className="border-b">
                <td className="p-3 text-xs">{new Date(tx.timestamp).toLocaleString()}</td>
                <td className="p-3">{tx.model_name}</td>
                <td className="p-3 font-mono text-xs">{tx.api_key_id.slice(0, 8)}...</td>
                <td className="p-3">{tx.input_tokens}</td>
                <td className="p-3">{tx.output_tokens}</td>
                <td className="p-3">${tx.cost_basis.toFixed(6)}</td>
                <td className="p-3">${tx.revenue_generated.toFixed(6)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="flex gap-3">
        {data.prev_cursor && (
          <button onClick={() => { setPrev(data.prev_cursor); setCursor(null); }}
            className="border border-slate-900 rounded px-3 py-2 text-sm">
            &#8592; Previous
          </button>
        )}
        {data.next_cursor && (
          <button onClick={() => { setCursor(data.next_cursor); setPrev(null); }}
            className="bg-slate-900 text-white rounded px-3 py-2 text-sm">
            Next &#8594;
          </button>
        )}
      </div>
    </div>
  );
}
