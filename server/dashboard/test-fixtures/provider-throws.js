module.exports = {
    async resolveUser() { throw new Error('upstream down'); },
};
