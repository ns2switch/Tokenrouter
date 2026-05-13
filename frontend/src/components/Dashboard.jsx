import { useState, useEffect } from 'react';
import * as api from '../api/client';

export default function Dashboard() {
  const [data, setData] = useState(null);
  const [error, setError] = useState('');

  useEffect(() => {
    loadData();
    const interval = setInterval(loadData, 15000);
    return () => clearInterval(interval);
  }, []);

  async function loadData() {
    try {
      const [dashboard, metrics] = await Promise.all([
        api.getDashboard(),
        api.getMetrics(),
      ]);
      setData({ dashboard, metrics });
    } catch (err) {
      setError(err.message);
    }
  }

  if (error) return <p className="text-red-600">{error}</p>;
  if (!data) return <p>Loading...</p>;

  const { dashboard: d, metrics: m } = data;
  const s = m.snapshot || {};
  const cards = [
    { label: 'Total Margin', value: `$${d?.metrics?.total_margin?.toFixed(4) || '0.0000'}` },
    { label: 'Input Tokens', value: d?.metrics?.total_input_tokens || 0 },
    { label: 'Output Tokens', value: d?.metrics?.total_output_tokens || 0 },
    { label: 'Transactions', value: d?.metrics?.tx_count || 0 },
    { label: 'Models Priced', value: d?.pricing_count || 0 },
    { label: 'Inflight Requests', value: m.inflight_requests },
    { label: 'Total Requests', value: s.requests_total || 0 },
    { label: 'Error Rate', value: errorRate(s.status_counts) },
  ];

  return (
    <div className="space-y-6">
      <div className="grid grid-cols-2 md:grid-cols-4 gap-4">
        {cards.map((c) => (
          <div key={c.label} className="bg-white rounded-xl shadow p-4">
            <p className="text-sm text-slate-500">{c.label}</p>
            <p className="text-2xl font-semibold text-slate-900">{c.value}</p>
          </div>
        ))}
      </div>

      {s.model_counts && s.model_counts.length > 0 && (
        <div className="bg-white rounded-xl shadow p-4">
          <h2 className="text-lg font-semibold mb-2">By Model</h2>
          <table className="w-full text-sm">
            <thead><tr className="text-left border-b"><th>Model</th><th>Requests</th></tr></thead>
            <tbody>
              {s.model_counts.map(([model, count]) => (
                <tr key={model} className="border-b"><td>{model}</td><td>{count}</td></tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {s.status_counts && s.status_counts.length > 0 && (
        <div className="bg-white rounded-xl shadow p-4">
          <h2 className="text-lg font-semibold mb-2">By Status</h2>
          <table className="w-full text-sm">
            <thead><tr className="text-left border-b"><th>Status</th><th>Count</th></tr></thead>
            <tbody>
              {s.status_counts.map(([code, count]) => (
                <tr key={code} className="border-b"><td>{code}</td><td>{count}</td></tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}

function errorRate(statusCounts) {
  const total = (statusCounts || []).reduce((s, [, c]) => s + c, 0);
  const errors = (statusCounts || [])
    .filter(([code]) => code >= 400)
    .reduce((s, [, c]) => s + c, 0);
  return total ? `${((errors / total) * 100).toFixed(1)}%` : '0%';
}
