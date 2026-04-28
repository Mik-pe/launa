import { createApp } from 'vue'
import './style.css'
import App from './App.vue'
import { initTooltipBounds } from './utils/tooltip'

initTooltipBounds()
createApp(App).mount('#app')
