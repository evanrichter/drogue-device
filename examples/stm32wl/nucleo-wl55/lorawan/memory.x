/* Memory for the NUCLEO-WL55JC2 */
MEMORY
{
  /* See section 4.3.1 "Flash memory organization" in the reference manual */
  FLASH : ORIGIN = 0x8000000, LENGTH = 256K
  RAM : ORIGIN = 0x20000000, LENGTH = 64K
}
