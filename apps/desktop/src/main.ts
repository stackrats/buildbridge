import { createApp } from 'vue';

import App from './App.vue';
import { vTip } from './lib/tooltip';
import { loadBackend } from './lib/backend';
import { applyTheme, loadTheme } from './lib/prefs';
import './style.css';

const forcedTheme = import.meta.env.DEV ? new URLSearchParams(location.search).get('theme') : null;
applyTheme(forcedTheme === 'light' || forcedTheme === 'dark' ? forcedTheme : loadTheme());

void loadBackend().then(() => {
    createApp(App).directive('tip', vTip).mount('#app');
});
