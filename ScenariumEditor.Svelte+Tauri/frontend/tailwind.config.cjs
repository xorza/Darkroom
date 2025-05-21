module.exports = {
    content: [
        './src/**/*.{html,js,svelte,ts}'
    ],
    theme: {
        extend: {
            colors: {
                primary: '#2563eb'
            }
        }
    },

    plugins: [require('daisyui')],
    daisyui: {
        themes: ['business']
    },
};
