# tcp-mitm

Generic TCP man-in-the-middle with a side tap

```text
sage: tcp_mitm [OPTIONS] --listen <LISTEN> --ports <PORTS> --server <SERVER> --tap <TAP>

Options:
  -d, --debug            
  -t, --trace            
      --listen <LISTEN>  
      --ports <PORTS>    
      --server <SERVER>  
      --tap <TAP>        
  -h, --help             Print help
```

Note that `--ports` can have a comma separated list and/or ranges.

Overlapping and/or repeated port numbers are combined.

For example, the following are valid `--ports` arguments:

```text
--ports 8080
--ports 8000-8009
--ports 8000-8009,8002 (makes no difference to previous)
--ports 8000,8080,1042
--ports 8000-8080,8090,9080,10800-10809
```

The server connection is made to the same destination port as client.

That is why server port cannot be set.

On tap connection, the destination port must be set.
