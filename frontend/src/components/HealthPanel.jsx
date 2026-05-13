import { useState, useEffect } from 'react';
import * as api from '../api/client';

export default function HealthPanel() {
  const [health, setHealth] = useState(null);
  const [error, setError] = useState('');

  useEffect(() => {
    loadHealth();
    const interval = setInterval(loadHealth, 10000);
    return () => clearInterval(interval);
  }, []);

  async function loadHealth() {
    try {
      setHealth(await api.getHealth());
    } catch (err) {
      setError(err.message);
    }
  }

  if (error) return <p className="text-red-600">{error}</p>;
  if (!health) return <p>Loading...</p>;

  return (
    <div className="space-y-6">
      <div className={`rounded-xl p-4 text-white ${health.ok ? 'bg-green-600' : 'bg-red-600'}`}>
        <p className="text-2xl font-bold">{health.ok ? 'All Systems OK' : 'Degraded'}</p>
      </div>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
        {health.checks.map((c) => (
          <div key={c.name} className="bg-white rounded-xl shadow p-4 flex items-center justify-between">
            <div>
              <p className="font-semibold text-slate-900">{c.name}</p>
              <p className="text-xs text-slate-500">{c.detail}</p>
            </div>
            <span className={`px-2 py-1 rounded text-xs font-bold ${c.ok ? 'bg-green-100 text-green-800' : 'bg-red-100 text-red-800'}`}>
              {c.ok ? 'OK' : 'FAIL'}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
