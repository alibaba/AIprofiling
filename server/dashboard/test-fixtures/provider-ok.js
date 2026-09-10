module.exports = {
    loginBase: 'https://example.test/login',
    logoutBase: 'https://example.test/logout',
    async resolveUser() { return { userId: 'alice', username: 'alice', nickname: 'Alice' }; },
};
