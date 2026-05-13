import { useState, useEffect } from 'react';
import * as api from './api/client';
import Login from './components/Login';
import Layout from './components/Layout';
import Dashboard from './components/Dashboard';
import KeysManager from './components/KeysManager';
import PricingManager from './components/PricingManager';
import ProvidersManager from './components/ProvidersManager';
import Transactions from './components/Transactions';
import DeadLetterPanel from './components/DeadLetterPanel';
import HealthPanel from './components/HealthPanel';

export default function App() {
  const [page, setPage] = useState('login');
  const [authenticated, setAuthenticated] = useState(false);

  useEffect(() => {
    api.getHealth()
      .then(() => setAuthenticated(true))
      .catch(() => setAuthenticated(false));
  }, []);

  function handleLogin(token) {
    return api.login(token).then(() => {
      setAuthenticated(true);
      setPage('dashboard');
    });
  }

  function handleLogout() {
    api.logout().finally(() => {
      setAuthenticated(false);
      setPage('login');
    });
  }

  if (!authenticated) {
    return <Login onLogin={handleLogin} />;
  }

  return (
    <Layout page={page} onNavigate={setPage} onLogout={handleLogout}>
      {page === 'dashboard' && <Dashboard />}
      {page === 'keys' && <KeysManager />}
      {page === 'pricing' && <PricingManager />}
      {page === 'providers' && <ProvidersManager />}
      {page === 'transactions' && <Transactions />}
      {page === 'dead-letter' && <DeadLetterPanel />}
      {page === 'health' && <HealthPanel />}
    </Layout>
  );
}
