import { withSupabase } from 'npm:@supabase/server@^1'

const SUPABASE_URL = Deno.env.get('SUPABASE_URL')
const SERVICE_ROLE_KEY = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')

const handler = async (req: Request): Promise<Response> => {
  const url = new URL(req.url)
  const token = url.searchParams.get('token')

  if (!token) return Response.json({ error: 'Missing token' }, { status: 400 })

  // 使用 service_role 权限更新数据库，无视 RLS
  const res = await fetch(`${SUPABASE_URL}/rest/v1/waitlist?confirmation_token=eq.${token}`, {
    method: 'PATCH',
    headers: {
      'apikey': SERVICE_ROLE_KEY,
      'Authorization': `Bearer ${SERVICE_ROLE_KEY}`,
      'Content-Type': 'application/json',
      'Prefer': 'return=representation',
    },
    body: JSON.stringify({ confirmed: true }),
  })

  const data = await res.json()

  if (!res.ok || data.length === 0) {
    return Response.json({ error: 'Invalid or expired token' }, { status: 400 })
  }

  return Response.json({ success: true })
}

export default { fetch: withSupabase({ auth: [] }, handler) }