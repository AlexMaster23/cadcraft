; Programmatic calls receive JSON objects as association lists with string keys.
(defun hello:run (params / pair name greeting)
  (setq pair (assoc "name" params))
  (setq name (if pair (cdr pair) "CADCraft"))
  (setq greeting (strcat "Hello, " name "!"))
  (princ (strcat "\n" greeting))
  greeting)

; Interactive extension commands define C:<manifest command id>.
(defun c:hello ()
  (hello:run nil)
  (princ))
