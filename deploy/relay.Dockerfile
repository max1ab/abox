FROM node:24-bookworm-slim AS build
WORKDIR /app
COPY package.json package-lock.json ./
COPY relay-server/package.json relay-server/package.json
COPY vendor/agmsg/server/package.json vendor/agmsg/server/package.json
RUN npm ci --ignore-scripts --no-audit --no-fund
COPY relay-server relay-server
COPY vendor/agmsg vendor/agmsg
RUN npm run build && npm prune --omit=dev --ignore-scripts --no-audit --no-fund

FROM node:24-bookworm-slim AS server
WORKDIR /app
ENV NODE_ENV=production HOST=0.0.0.0 PORT=8787
COPY --from=build --chown=node:node /app/package.json ./package.json
COPY --from=build --chown=node:node /app/node_modules ./node_modules
COPY --from=build --chown=node:node /app/relay-server/package.json ./relay-server/package.json
COPY --from=build --chown=node:node /app/relay-server/dist ./relay-server/dist
COPY --from=build --chown=node:node /app/vendor/agmsg/server/package.json ./vendor/agmsg/server/package.json
COPY --from=build --chown=node:node /app/vendor/agmsg/server/dist ./vendor/agmsg/server/dist
COPY --from=build --chown=node:node /app/vendor/agmsg/LICENSE /app/vendor/agmsg/VERSION ./vendor/agmsg/
USER node
EXPOSE 8787
HEALTHCHECK --interval=15s --timeout=5s --start-period=20s --retries=3 CMD node -e "fetch('http://127.0.0.1:8787/v1/agent-mail/health').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))"
CMD ["node", "relay-server/dist/src/main.js"]
