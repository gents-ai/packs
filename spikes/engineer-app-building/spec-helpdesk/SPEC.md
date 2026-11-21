# Help desk

Ticket data for a small support team, kept on this node.

## Source

`tickets.csv` in the workspace, with a header row and these columns:

`ticket_id, customer_id, customer_name, subject, body, opened_at, status`

`opened_at` is UTC in ISO 8601. `status` is `open` or `closed`. Rows are added to the file over time and a row's status can change. A `ticket_id` never repeats.

## Collections

- `DeskCustomer`: `customer_id` (unique), `name`.
- `DeskTicket`: `ticket_id` (unique), `customer_id`, `subject`, `body`, `opened_at`, `status`, `priority` (`urgent`, `normal` or `low`), `summary` (one line, at most 120 characters).

## What it does

- Importing the file makes every row one `DeskTicket` and every customer one `DeskCustomer`. Importing again creates no duplicates and applies status changes. I can ask for an import at any time.
- An AI agent reads each new ticket's subject and body and sets its `priority` and `summary`. A ticket is triaged once, without anyone asking.
- Tickets can be searched by words in their subject or body.
- A page in the app shows open tickets grouped by customer, counts by priority, and marks tickets that have been open more than 48 hours.
