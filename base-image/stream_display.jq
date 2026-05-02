fromjson? // . |
if type == "string" then .
elif .type == "assistant" then
  .message.content[]? |
  if .type == "text" then
    "\u001b[0;37m" + .text + "\u001b[0m"
  elif .type == "tool_use" then
    if .name == "Read" or .name == "Glob" or .name == "Agent" then
      empty
    else
      "\u001b[2;37m  " + .name +
      (if .name == "Bash" then " → " + (.input.command // "" | tostring)
       elif .name == "Write" then " → " + (.input.file_path // .input.path // "" | tostring)
       elif .name == "Edit" then " → " + (.input.file_path // .input.path // "" | tostring)
       elif (.input | length) > 0 then " → " + (.input | keys[0] // "" | tostring) + ": " + (.input[.input | keys[0]] // "" | tostring | .[0:80])
       else ""
       end) + "\u001b[0m"
    end
  else empty
  end
else empty
end
