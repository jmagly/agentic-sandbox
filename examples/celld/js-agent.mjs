export default {
  async fetch(request) {
    const trace = request.headers.get("traceparent") || crypto.randomUUID();
    return Response.json({ runtime: "worker-celld", language: "javascript", trace });
  },
};
