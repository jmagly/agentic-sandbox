/** Constrained A2A adapter: HTTP/JSON only, with no process or workspace APIs. */
export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/.well-known/agent-card.json") {
      return Response.json({
        name: "worker-celld reference agent",
        description: "A2A reference implemented inside a constrained Worker isolate",
        url: url.origin,
        version: "1.0.0",
        capabilities: { streaming: false, pushNotifications: false, stateTransitionHistory: true },
        defaultInputModes: ["application/json", "text/plain"],
        defaultOutputModes: ["application/json"],
        skills: [{ id: "echo", name: "Echo", description: "Returns the submitted message" }],
      });
    }
    if (url.pathname !== "/a2a" || request.method !== "POST") return Response.json({ error: { code: -32601, message: "method not found" } }, { status: 404 });
    const rpc = await request.json();
    if (rpc.method === "message/send") {
      const id = crypto.randomUUID();
      const task = { id, contextId: rpc.params?.message?.contextId || id, status: { state: "completed", timestamp: new Date().toISOString() }, artifacts: [{ artifactId: crypto.randomUUID(), parts: [{ kind: "data", data: { echo: rpc.params?.message || null, runtime: "worker-celld" } }] }] };
      await env.TASKS.put(`task:${id}`, JSON.stringify(task));
      return Response.json({ jsonrpc: "2.0", id: rpc.id, result: task });
    }
    if (rpc.method === "tasks/get") {
      const task = await env.TASKS.get(`task:${rpc.params?.id}`);
      return task ? Response.json({ jsonrpc: "2.0", id: rpc.id, result: JSON.parse(task) }) : Response.json({ jsonrpc: "2.0", id: rpc.id, error: { code: -32001, message: "task not found" } }, { status: 404 });
    }
    return Response.json({ jsonrpc: "2.0", id: rpc.id, error: { code: -32601, message: "method not found" } }, { status: 400 });
  },
};
