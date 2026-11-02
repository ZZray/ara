# Independent service; no LabSlot account, database or application dependency.
location = /ssh-tunnel {
    return 308 /ssh-tunnel/;
}
location ^~ /ssh-tunnel/ {
    limit_req zone=ry_ssh_tunnel burst=20 nodelay;
    limit_req_status 429;
    proxy_pass http://127.0.0.1:50122/;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
    proxy_set_header Host $host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_read_timeout 90s;
    proxy_send_timeout 90s;
    proxy_connect_timeout 5s;
    proxy_buffering off;
    proxy_cache off;
    proxy_next_upstream off;
    client_max_body_size 32k;
    # Pair credentials must never enter request/access logs.
    access_log off;
}
