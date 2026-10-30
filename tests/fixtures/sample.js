// rsray test fixture: every endpoint below must be found, and the
// junk strings must NOT appear in the API list.
const API = "https://api.example.com";
const ADMIN = "/api/v1/admin/users";

fetch("/api/v1/users").then(r => r.json());
fetch("/api/" + "v2/orders");
fetch(API + "/products");
fetch(`/api/v1/users/${userId}/orders`);
fetch(ADMIN, { method: "POST" });
axios.get("/api/v1/internal/debug");
new WebSocket("wss://example.com/socket");

const logo = "/static/logo.png";
const greeting = "hello world";
const colour = "red";
